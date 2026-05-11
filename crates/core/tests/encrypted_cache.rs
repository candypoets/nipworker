mod common;

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use nipworker_core::crypto::signers::PrivateKeySigner;
use nipworker_core::generated::nostr::fb;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::types::network::Request;
use tokio::task::LocalSet;

const USER_SECRET: &str = "f7e69dd87239da6a828fb9a2fbf481b5b9e147edb848497620e8dc6f5ec10a0a";
const PEER_SECRET: &str = "791541b690c9d83c1265ab5e7d44078c52c34816d087cbac9cd204527a54f708";
const SIGNATURE: &str = "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001";

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

async fn drain_crypto_ack(
	event_sink_rx: &mut futures::channel::mpsc::Receiver<(String, Vec<u8>)>,
) {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
	while tokio::time::Instant::now() < deadline {
		match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
			Ok(Some((sub_id, _))) if sub_id == "crypto" => return,
			Ok(Some(_)) => continue,
			_ => continue,
		}
	}
	panic!("timed out waiting for signer ack");
}

async fn next_parsed_event(
	subscription_id: &str,
	event_sink_rx: &mut futures::channel::mpsc::Receiver<(String, Vec<u8>)>,
) -> fb::ParsedEvent<'static> {
	let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
	while tokio::time::Instant::now() < deadline {
		match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
			Ok(Some((sub_id, bytes))) if sub_id == subscription_id => {
				let leaked: &'static [u8] = Box::leak(bytes.into_boxed_slice());
				let wm = flatbuffers::root::<fb::WorkerMessage>(leaked)
					.expect("valid WorkerMessage");
				if wm.content_type() == fb::Message::ParsedEvent {
					return wm.content_as_parsed_event().expect("ParsedEvent content");
				}
			}
			Ok(Some(_)) => continue,
			_ => continue,
		}
	}
	panic!("timed out waiting for parsed event");
}

#[tokio::test]
async fn test_cache_only_kind4_decrypts_after_private_key_signer_is_set() {
	let local = LocalSet::new();
	local
		.run_until(async {
			let user = PrivateKeySigner::new(USER_SECRET).unwrap();
			let peer = PrivateKeySigner::new(PEER_SECRET).unwrap();
			let user_pubkey = user.get_public_key().unwrap();
			let peer_pubkey = peer.get_public_key().unwrap();
			let plaintext = "secret dm from cache";
			let encrypted = peer.nip04_encrypt(&user_pubkey, plaintext).unwrap();
			let event_id = "0000000000000000000000000000000000000000000000000000000000000401";

			let encrypted_event = common::build_nostr_event_worker_message_with_tags(
				"cache",
				"wss://r1",
				event_id,
				&peer_pubkey,
				4,
				&encrypted,
				vec![vec!["p".to_string(), user_pubkey.clone()]],
				1234567890,
				SIGNATURE,
			);

			let storage = Arc::new(common::MockStorage::with_query_results(vec![vec![
				encrypted_event,
			]]));
			let transport = Arc::new(common::MockRelayTransport::new());
			let (event_sink_tx, mut event_sink_rx) =
				futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);
			let engine = NostrEngine::new(transport, storage, event_sink_tx);

			engine
				.handle_message(&build_set_private_key_message(USER_SECRET))
				.await
				.unwrap();
			drain_crypto_ack(&mut event_sink_rx).await;

			engine
				.subscribe(
					"encrypted-kind4".to_string(),
					vec![Request {
						ids: vec![event_id.to_string()],
						kinds: vec![4],
						cache_only: true,
						cache_first: true,
						relays: vec!["wss://r1".to_string()],
						..Default::default()
					}],
				)
				.await
				.unwrap();

			let parsed = next_parsed_event("encrypted-kind4", &mut event_sink_rx).await;
			assert_eq!(parsed.kind(), 4);
			let kind4 = parsed.parsed_as_kind_4_parsed().expect("kind4 parsed");
			assert_eq!(kind4.decrypted_content(), Some(plaintext));
		})
		.await;
}

#[tokio::test]
async fn test_cache_only_kind7375_decrypts_after_private_key_signer_is_set() {
	let local = LocalSet::new();
	local
		.run_until(async {
			let user = PrivateKeySigner::new(USER_SECRET).unwrap();
			let user_pubkey = user.get_public_key().unwrap();
			let token_content = r#"{"mint":"https://mint.example","proofs":[{"amount":21,"id":"proof-id","secret":"cashu-secret","C":"02aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
			let encrypted = user.nip44_encrypt("", token_content).unwrap();
			let event_id = "0000000000000000000000000000000000000000000000000000000000007375";

			let encrypted_event = common::build_nostr_event_worker_message_with_tags(
				"cache",
				"wss://r1",
				event_id,
				&user_pubkey,
				7375,
				&encrypted,
				vec![],
				1234567890,
				SIGNATURE,
			);

			let storage = Arc::new(common::MockStorage::with_query_results(vec![vec![
				encrypted_event,
			]]));
			let transport = Arc::new(common::MockRelayTransport::new());
			let (event_sink_tx, mut event_sink_rx) =
				futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);
			let engine = NostrEngine::new(transport, storage, event_sink_tx);

			engine
				.handle_message(&build_set_private_key_message(USER_SECRET))
				.await
				.unwrap();
			drain_crypto_ack(&mut event_sink_rx).await;

			engine
				.subscribe(
					"encrypted-kind7375".to_string(),
					vec![Request {
						ids: vec![event_id.to_string()],
						kinds: vec![7375],
						cache_only: true,
						cache_first: true,
						relays: vec!["wss://r1".to_string()],
						..Default::default()
					}],
				)
				.await
				.unwrap();

			let parsed = next_parsed_event("encrypted-kind7375", &mut event_sink_rx).await;
			assert_eq!(parsed.kind(), 7375);
			let kind7375 = parsed
				.parsed_as_kind_7375_parsed()
				.expect("kind7375 parsed");
			assert!(kind7375.decrypted());
			assert_eq!(kind7375.mint_url(), "https://mint.example");
			assert_eq!(kind7375.proofs().len(), 1);
			assert_eq!(kind7375.proofs().get(0).amount(), 21);
		})
		.await;
}
