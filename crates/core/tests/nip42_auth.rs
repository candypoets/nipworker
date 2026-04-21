mod common;

use std::sync::Arc;
use std::time::Duration;
use futures::StreamExt;
use nipworker_core::generated::nostr::fb;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::types::network::Request;
use tokio::task::LocalSet;

const PUBKEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SIGNATURE: &str = "0000000000000000000000000000000000000000000000000000000000000002";

#[tokio::test]
async fn test_nip42_auth_roundtrip() {
	let local = LocalSet::new();
	local
		.run_until(async {
			let transport = Arc::new(common::MockRelayTransport::new());
			let storage = Arc::new(common::MockStorage::new());
			let signer = Arc::new(common::MockSigner::new(PUBKEY, SIGNATURE));
			let (event_sink_tx, mut event_sink_rx) =
				futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

			let engine = NostrEngine::new(
				transport.clone(),
				storage.clone(),
				signer.clone(),
				event_sink_tx,
			);

			let request = Request {
				relays: vec!["wss://r".to_string()],
				..Default::default()
			};
			engine
				.subscribe("sub1".to_string(), vec![request])
				.await
				.unwrap();

			tokio::time::sleep(Duration::from_millis(200)).await;

			// Relay challenges with AUTH.
			transport.invoke_message_callback(
				"wss://r",
				r#"["AUTH","challenge123"]"#.to_string(),
			);

			// Wait for AUTH response frame to be sent back to relay.
			let mut found_auth = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				tokio::time::sleep(Duration::from_millis(50)).await;
				let frames = transport.get_sent_frames();
				if frames.iter().any(|(url, frame)| {
					url == "wss://r" && frame.starts_with(r#"["AUTH","#)
				}) {
					found_auth = true;
					break;
				}
			}
			assert!(found_auth, "AUTH frame should be sent to relay");

			// Relay accepts the AUTH.
			transport.invoke_message_callback(
				"wss://r",
				r#"["OK","auth-id","true"]"#.to_string(),
			);

			// Verify the system remains functional by injecting an EVENT.
			let event_json = common::make_event_json(
				"0000000000000000000000000000000000000000000000000000000000000003",
				PUBKEY,
				1,
				"post-auth event",
				1234567890,
				SIGNATURE,
			);
			transport.invoke_message_callback(
				"wss://r",
				format!(r#"["EVENT","sub1",{}]"#, event_json),
			);

			let mut found_event = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
					Ok(Some((sub_id, bytes))) if sub_id == "sub1" => {
						let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
							.expect("valid WorkerMessage");
						if wm.type_() == fb::MessageType::ParsedNostrEvent
							|| wm.type_() == fb::MessageType::NostrEvent
						{
							found_event = true;
							break;
						}
					}
					Ok(Some(_)) => continue,
					_ => continue,
				}
			}
			assert!(found_event, "Event should arrive after auth roundtrip");
		})
		.await;
}
