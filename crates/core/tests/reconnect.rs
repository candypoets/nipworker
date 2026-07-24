mod common;
use std::sync::Arc;
use std::time::Duration;

use nipworker_core::service::engine::NostrEngine;
use nipworker_core::traits::TransportStatus;
use nipworker_core::types::network::Request;
use tokio::task::LocalSet;

#[tokio::test]
async fn test_unexpected_transport_close_enters_cooldown() {
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

			// A new subscription during cooldown must not immediately hammer the relay.
			let request2 = Request {
				relays: vec!["wss://r".to_string()],
				..Default::default()
			};
			engine
				.subscribe("sub2".to_string(), vec![request2])
				.await
				.unwrap();

			tokio::time::sleep(Duration::from_millis(200)).await;
			let calls_during_cooldown = transport.get_calls();
			let connect_count = calls_during_cooldown
				.iter()
				.filter(|c| matches!(c, common::TransportCall::Connect(url) if url == "wss://r"))
				.count();
			assert_eq!(
				connect_count, 1,
				"unexpected close cooldown should suppress an immediate reconnect"
			);
		})
		.await;
}
