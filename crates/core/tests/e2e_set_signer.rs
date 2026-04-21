mod common;

use std::sync::Arc;
use std::time::Duration;
use futures::StreamExt;
use nipworker_core::generated::nostr::fb;
use nipworker_core::service::engine::NostrEngine;
use tokio::task::LocalSet;

const PUBKEY_A: &str = "000000000000000000000000000000000000000000000000000000000000000a";
const PUBKEY_B: &str = "000000000000000000000000000000000000000000000000000000000000000b";
const SIG_A: &str = "00000000000000000000000000000000000000000000000000000000000000a1";
const SIG_B: &str = "00000000000000000000000000000000000000000000000000000000000000b1";

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

/// Poll the event sink for a crypto response and return the parsed JSON result.
async fn poll_crypto_response(
    event_sink_rx: &mut futures::channel::mpsc::Receiver<(String, Vec<u8>)>,
) -> serde_json::Value {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
            Ok(Some((sub_id, bytes))) if sub_id == "crypto" => {
                // Crypto worker sends a WorkerMessage FlatBuffer with MessageType::Raw.
                let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
                    .expect("valid WorkerMessage from crypto worker");
                if wm.type_() == fb::MessageType::Raw {
                    if let Some(raw) = wm.content_as_raw() {
                        let json_str = raw.raw();
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) {
                            return val;
                        }
                    }
                }
            }
            Ok(Some(_)) => continue,
            _ => continue,
        }
    }
    panic!("Timed out waiting for crypto response");
}

#[tokio::test]
async fn test_set_signer_hot_swap() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let transport = Arc::new(common::MockRelayTransport::new());
            let storage = Arc::new(common::MockStorage::new());
            let signer_a = Arc::new(common::MockSigner::new(PUBKEY_A, SIG_A));
            let (event_sink_tx, mut event_sink_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

            let engine = NostrEngine::new(
                transport.clone(),
                storage.clone(),
                signer_a.clone(),
                event_sink_tx,
            );

            // 1) Initial signer: request public key.
            engine
                .handle_message(&build_get_public_key_message())
                .await
                .unwrap();

            let resp = poll_crypto_response(&mut event_sink_rx).await;
            assert_eq!(
                resp["result"].as_str(),
                Some(PUBKEY_A),
                "Initial signer should return pubkey A"
            );

            // 2) Hot-swap to signer B.
            let signer_b = Arc::new(common::MockSigner::new(PUBKEY_B, SIG_B));
            engine.set_signer(signer_b.clone()).await;

            // 3) New signer: request public key again.
            engine
                .handle_message(&build_get_public_key_message())
                .await
                .unwrap();

            let resp = poll_crypto_response(&mut event_sink_rx).await;
            assert_eq!(
                resp["result"].as_str(),
                Some(PUBKEY_B),
                "Swapped signer should return pubkey B"
            );

            // 4) Verify the new signer was actually invoked.
            let b_calls = signer_b.calls.lock().unwrap();
            assert!(
                b_calls.iter().any(|c| matches!(c, common::SignerCall::GetPublicKey)),
                "Signer B should have been called for get_public_key"
            );
        })
        .await;
}
