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
async fn test_subscribe_receive_eose() {
	let local = LocalSet::new();
	local
		.run_until(async {
			let transport = Arc::new(common::MockRelayTransport::new());
			let storage = Arc::new(common::MockStorage::new());
			
			let (event_sink_tx, mut event_sink_rx) =
				futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

			let engine = NostrEngine::new(
				transport.clone(),
				storage.clone(),
				event_sink_tx,
			);

			let request = Request {
				relays: vec!["wss://r1".to_string()],
				..Default::default()
			};
			engine
				.subscribe("sub1".to_string(), vec![request])
				.await
				.unwrap();

			// Allow connect + REQ to propagate across workers.
			tokio::time::sleep(Duration::from_millis(2000)).await;

			let calls = transport.get_calls();
			assert!(
				calls.iter().any(|c| matches!(c, common::TransportCall::Connect(url) if url == "wss://r1")),
				"relay should have been connected"
			);
			assert!(
				calls.iter().any(|c| matches!(c, common::TransportCall::Send(url, frame) if url == "wss://r1" && frame.contains("REQ"))),
				"REQ should have been sent"
			);

			// Inject an EVENT from the relay.
			let event_json = common::make_event_json(
				"0000000000000000000000000000000000000000000000000000000000000003",
				PUBKEY,
				1,
				"hello integration test",
				1234567890,
				SIGNATURE,
			);
			transport.invoke_message_callback(
				"wss://r1",
				format!(r#"["EVENT","sub1",{}]"#, event_json),
			);

			// Poll event sink until the event arrives (skip status/EOCE from cache).
			let mut found_event = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
					Ok(Some((sub_id, bytes))) if sub_id == "sub1" => {
						let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
							.expect("valid WorkerMessage");
						match wm.type_() {
							fb::MessageType::ParsedNostrEvent => {
								found_event = true;
								break;
							}
							fb::MessageType::Eoce | fb::MessageType::ConnectionStatus => {
								// EOCE from empty cache, or connection status updates
								continue;
							}
							other => {
								panic!("unexpected message type at event_sink: {:?}", other);
							}
						}
					}
					Ok(Some(_)) => continue,
					_ => continue,
				}
			}
			assert!(found_event, "Expected ParsedNostrEvent at event_sink");

			// Verify the event was persisted to cache (SaveToDbPipe -> CacheWorker -> Storage).
			let mut found_persisted = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
			while tokio::time::Instant::now() < deadline {
				tokio::time::sleep(Duration::from_millis(50)).await;
				if !storage.get_persisted().is_empty() {
					found_persisted = true;
					break;
				}
			}
			assert!(found_persisted, "Expected event to be persisted to storage");

			// Inject EOSE.
			transport.invoke_message_callback("wss://r1", r#"["EOSE","sub1"]"#.to_string());

			let mut found_eose = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
					Ok(Some((sub_id, bytes))) if sub_id == "sub1" => {
						let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
							.expect("valid WorkerMessage");
						if wm.type_() == fb::MessageType::ConnectionStatus {
							if let Some(cs) = wm.content_as_connection_status() {
								if cs.status() == "EOSE" {
									found_eose = true;
									break;
								}
							}
						}
					}
					Ok(Some(_)) => continue,
					_ => continue,
				}
			}
			assert!(found_eose, "Expected EOSE at event_sink");
		})
		.await;
}
