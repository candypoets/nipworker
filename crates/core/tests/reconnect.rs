mod common;
use std::sync::Arc;

use std::time::Duration;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::traits::TransportStatus;
use nipworker_core::types::network::Request;
use tokio::task::LocalSet;

const PUBKEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SIGNATURE: &str = "0000000000000000000000000000000000000000000000000000000000000002";

#[tokio::test]
async fn test_reconnect_after_transport_close() {
	let local = LocalSet::new();
	local
		.run_until(async {
			let transport = Arc::new(common::MockRelayTransport::new());
			let storage = Arc::new(common::MockStorage::new());
			
			let (event_sink_tx, _event_sink_rx) =
				futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

			let engine = NostrEngine::new(
				transport.clone(),
				storage.clone(),
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

			// Verify initial connect + REQ.
			let calls_before = transport.get_calls();
			assert!(
				calls_before.iter().any(|c| matches!(c, common::TransportCall::Connect(url) if url == "wss://r")),
				"initial connect expected"
			);
			assert!(
				calls_before.iter().any(|c| matches!(c, common::TransportCall::Send(url, frame) if url == "wss://r" && frame.contains("REQ"))),
				"initial REQ expected"
			);

			// Simulate unexpected transport close.
			transport.invoke_status_callback(
				"wss://r",
				TransportStatus::Closed {
					url: "wss://r".to_string(),
				},
			);

			tokio::time::sleep(Duration::from_millis(100)).await;

			// Trigger a new subscription on the same relay.
			let request2 = Request {
				relays: vec!["wss://r".to_string()],
				..Default::default()
			};
			engine
				.subscribe("sub2".to_string(), vec![request2])
				.await
				.unwrap();

			// Poll until reconnect and new REQ are observed.
			let mut found_reconnect = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				tokio::time::sleep(Duration::from_millis(50)).await;
				let calls = transport.get_calls();
				let connect_count = calls
					.iter()
					.filter(|c| matches!(c, common::TransportCall::Connect(url) if url == "wss://r"))
					.count();
				if connect_count >= 2 {
					let req_sent = calls.iter().any(|c| matches!(
						c,
						common::TransportCall::Send(url, frame)
							if url == "wss://r" && frame.contains("REQ") && frame.contains("sub2")
					));
					if req_sent {
						found_reconnect = true;
						break;
					}
				}
			}
			assert!(
				found_reconnect,
				"Expected reconnect and new REQ after transport close"
			);
		})
		.await;
}
