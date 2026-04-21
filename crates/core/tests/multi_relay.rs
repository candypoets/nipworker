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
async fn test_multi_relay_partial_eose() {
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
				relays: vec!["wss://r1".to_string(), "wss://r2".to_string()],
				..Default::default()
			};
			engine
				.subscribe("multi".to_string(), vec![request])
				.await
				.unwrap();

			tokio::time::sleep(Duration::from_millis(200)).await;

			// Verify both relays were targeted.
			let calls = transport.get_calls();
			assert!(
				calls.iter().any(|c| matches!(c, common::TransportCall::Connect(url) if url == "wss://r1")),
				"r1 should connect"
			);
			assert!(
				calls.iter().any(|c| matches!(c, common::TransportCall::Connect(url) if url == "wss://r2")),
				"r2 should connect"
			);
			assert!(
				calls.iter().any(|c| matches!(c, common::TransportCall::Send(url, frame) if url == "wss://r1" && frame.contains("REQ"))),
				"r1 REQ expected"
			);
			assert!(
				calls.iter().any(|c| matches!(c, common::TransportCall::Send(url, frame) if url == "wss://r2" && frame.contains("REQ"))),
				"r2 REQ expected"
			);

			// Inject EVENT + EOSE from r1 first.
			let event_json = common::make_event_json(
				"0000000000000000000000000000000000000000000000000000000000000003",
				PUBKEY,
				1,
				"from r1",
				1234567890,
				SIGNATURE,
			);
			transport.invoke_message_callback(
				"wss://r1",
				format!(r#"["EVENT","multi",{}]"#, event_json),
			);
			transport.invoke_message_callback(
				"wss://r1",
				r#"["EOSE","multi"]"#.to_string(),
			);

			let mut found_r1_eose = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
					Ok(Some((sub_id, bytes))) if sub_id == "multi" => {
						let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
							.expect("valid WorkerMessage");
						if wm.type_() == fb::MessageType::ConnectionStatus {
							if let Some(cs) = wm.content_as_connection_status() {
								if cs.status() == "EOSE" && cs.relay_url() == "wss://r1" {
									found_r1_eose = true;
									break;
								}
							}
						}
					}
					Ok(Some(_)) => continue,
					_ => continue,
				}
			}
			assert!(found_r1_eose, "Expected partial EOSE from r1");

			// Inject EVENT + EOSE from r2.
			let event_json2 = common::make_event_json(
				"0000000000000000000000000000000000000000000000000000000000000004",
				PUBKEY,
				1,
				"from r2",
				1234567891,
				SIGNATURE,
			);
			transport.invoke_message_callback(
				"wss://r2",
				format!(r#"["EVENT","multi",{}]"#, event_json2),
			);
			transport.invoke_message_callback(
				"wss://r2",
				r#"["EOSE","multi"]"#.to_string(),
			);

			let mut found_r2_eose = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
					Ok(Some((sub_id, bytes))) if sub_id == "multi" => {
						let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
							.expect("valid WorkerMessage");
						if wm.type_() == fb::MessageType::ConnectionStatus {
							if let Some(cs) = wm.content_as_connection_status() {
								if cs.status() == "EOSE" && cs.relay_url() == "wss://r2" {
									found_r2_eose = true;
									break;
								}
							}
						}
					}
					Ok(Some(_)) => continue,
					_ => continue,
				}
			}
			assert!(found_r2_eose, "Expected final EOSE from r2");
		})
		.await;
}
