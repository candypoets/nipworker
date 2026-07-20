mod common;

use futures::StreamExt;
use nipworker_core::generated::nostr::fb;
use nipworker_core::service::engine::NostrEngine;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::LocalSet;

const SECRET_A: &str = "f7e69dd87239da6a828fb9a2fbf481b5b9e147edb848497620e8dc6f5ec10a0a";
const SECRET_B: &str = "791541b690c9d83c1265ab5e7d44078c52c34816d087cbac9cd204527a54f708";

/// Build a FlatBuffers GetPublicKey MainMessage.
fn build_get_public_key_message() -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let get_pk = fb::GetPublicKey::create(&mut builder, &fb::GetPublicKeyArgs {});
    let main_msg = fb::MainMessage::create(
        &mut builder,
        &fb::MainMessageArgs {
            content_type: fb::MainContent::GetPublicKey,
            content: Some(get_pk.as_union_value()),
        },
    );
    builder.finish(main_msg, None);
    builder.finished_data().to_vec()
}

/// Build a FlatBuffers SetSigner(PrivateKey) MainMessage.
fn build_set_private_key_message(secret: &str) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let mut pk = fb::PrivateKeyT::default();
    pk.private_key = secret.to_string();
    let signer_type = fb::SignerTypeT::PrivateKey(Box::new(pk));
    let signer_offset = signer_type.pack(&mut builder);
    let set_signer = fb::SetSigner::create(
        &mut builder,
        &fb::SetSignerArgs {
            signer_type_type: fb::SignerType::PrivateKey,
            signer_type: signer_offset,
        },
    );
    let main_msg = fb::MainMessage::create(
        &mut builder,
        &fb::MainMessageArgs {
            content_type: fb::MainContent::SetSigner,
            content: Some(set_signer.as_union_value()),
        },
    );
    builder.finish(main_msg, None);
    builder.finished_data().to_vec()
}

/// Poll the event sink for the next crypto response WorkerMessage.
/// Consumes *any* crypto sub_id response (including SetSigner acks).
async fn poll_crypto_response(
    event_sink_rx: &mut futures::channel::mpsc::Receiver<(String, Vec<u8>)>,
) -> Vec<u8> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
            Ok(Some((sub_id, bytes))) if sub_id == "crypto" => return bytes,
            Ok(Some(_)) => continue,
            _ => continue,
        }
    }
    panic!("Timed out waiting for crypto response");
}

/// Drain all pending crypto responses until a Pubkey response is found.
async fn drain_until_get_pubkey(
    event_sink_rx: &mut futures::channel::mpsc::Receiver<(String, Vec<u8>)>,
) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
            Ok(Some((sub_id, bytes))) if sub_id == "crypto" => {
                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
                    .expect("valid WorkerMessage from crypto worker");
                if wm.type_() == fb::MessageType::Pubkey {
                    let pubkey = wm.content_as_pubkey().expect("Pubkey content");
                    assert!(
                        pubkey.error().is_none(),
                        "get_public_key should not error, got {:?}",
                        pubkey.error()
                    );
                    return pubkey.pubkey().to_string();
                }
                // Otherwise it's a SetSignerResponse ack or something else — drain it
            }
            Ok(Some(_)) => continue,
            _ => continue,
        }
    }
    panic!("Timed out waiting for get_public_key response");
}

#[tokio::test]
async fn test_set_signer_hot_swap() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let transport = Arc::new(common::MockRelayTransport::new());
            let storage = Arc::new(common::MockStorage::new());
            let (event_sink_tx, mut event_sink_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

            let engine = NostrEngine::new(transport.clone(), storage.clone(), event_sink_tx);

            // 1) No signer configured yet: request public key should error.
            engine
                .handle_message(&build_get_public_key_message())
                .await
                .unwrap();

            let resp = poll_crypto_response(&mut event_sink_rx).await;
            let wm = flatbuffers::root::<fb::WorkerMessage>(&resp)
                .expect("valid WorkerMessage from crypto worker");
            assert_eq!(wm.type_(), fb::MessageType::Pubkey);
            let pubkey = wm.content_as_pubkey().expect("Pubkey content");
            assert!(
                pubkey.error().is_some(),
                "GetPublicKey should error when no signer configured"
            );

            // 2) Set signer A via FlatBuffers message.
            engine
                .handle_message(&build_set_private_key_message(SECRET_A))
                .await
                .unwrap();

            // Drain the SetSigner ack so it doesn't shadow the next GetPublicKey response.
            let _ = poll_crypto_response(&mut event_sink_rx).await;

            // 3) Request public key: should return a valid pubkey (signer A).
            engine
                .handle_message(&build_get_public_key_message())
                .await
                .unwrap();

            let pubkey_a = drain_until_get_pubkey(&mut event_sink_rx).await;
            assert!(
                !pubkey_a.is_empty(),
                "Signer A should return a non-empty pubkey"
            );

            // 4) Hot-swap to signer B via another SetSigner message.
            engine
                .handle_message(&build_set_private_key_message(SECRET_B))
                .await
                .unwrap();

            // Drain the SetSigner ack.
            let _ = poll_crypto_response(&mut event_sink_rx).await;

            // 5) Request public key again: should return a DIFFERENT pubkey (signer B).
            engine
                .handle_message(&build_get_public_key_message())
                .await
                .unwrap();

            let pubkey_b = drain_until_get_pubkey(&mut event_sink_rx).await;
            assert!(
                !pubkey_b.is_empty(),
                "Signer B should return a non-empty pubkey"
            );

            // The key assertion: the two pubkeys must be different.
            assert_ne!(
                pubkey_a, pubkey_b,
                "Hot-swapped signer must produce a different public key"
            );
        })
        .await;
}
