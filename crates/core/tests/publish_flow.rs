mod common;

use std::sync::Arc;
use std::time::Duration;
use futures::StreamExt;
use nipworker_core::generated::nostr::fb;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::types::nostr::Template;
use tokio::task::LocalSet;

const PUBKEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";
const SIGNATURE: &str = "0000000000000000000000000000000000000000000000000000000000000002";

#[tokio::test]
async fn test_publish_flow_reaches_relay() {
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

			let template = Template {
				kind: 1,
				content: "published note".to_string(),
				tags: vec![],
				created_at: 1234567890,
			};

			engine
				.publish(
					"pub1".to_string(),
					&template,
					vec!["wss://r".to_string()],
					vec![],
				)
				.await
				.unwrap();

			// Poll until the EVENT frame is sent to the relay.
			let mut found_event_frame = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				tokio::time::sleep(Duration::from_millis(50)).await;
				let frames = transport.get_sent_frames();
				if frames.iter().any(|(url, frame)| {
					url == "wss://r" && frame.starts_with(r#"["EVENT","#)
				}) {
					found_event_frame = true;
					break;
				}
			}
			assert!(found_event_frame, "EVENT frame should reach relay");

			// Verify the signer was asked to sign.
			let sign_calls: Vec<_> = signer
				.calls
				.lock()
				.unwrap()
				.iter()
				.filter_map(|c| match c {
					common::SignerCall::SignEvent(json) => Some(json.clone()),
					_ => None,
				})
				.collect();
			assert!(!sign_calls.is_empty(), "Signer should have been invoked");

			// Wait for synthetic SENT status to reach event sink.
			let mut found_sent = false;
			let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
			while tokio::time::Instant::now() < deadline {
				match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
					Ok(Some((sub_id, bytes))) if sub_id == "pub1" => {
						let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
							.expect("valid WorkerMessage");
						if wm.type_() == fb::MessageType::ConnectionStatus {
							if let Some(cs) = wm.content_as_connection_status() {
								if cs.status() == "SENT" {
									found_sent = true;
									break;
								}
							}
						}
					}
					Ok(Some(_)) => continue,
					_ => continue,
				}
			}
			assert!(found_sent, "SENT status should reach event_sink");
		})
		.await;
}
