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
async fn test_cache_hit_then_network_event() {
    let local = LocalSet::new();
    local
        .run_until(async {
            // Pre-populate storage with one cached event.
            let cached_event = common::build_nostr_event_worker_message(
                "sub1",
                "wss://r1",
                "00000000000000000000000000000000000000000000000000000000000000c1",
                PUBKEY,
                1,
                "cached hello",
                1234560000,
                SIGNATURE,
            );
            let storage = Arc::new(common::MockStorage::with_query_results(vec![vec![
                cached_event.clone(),
            ]]));

            let transport = Arc::new(common::MockRelayTransport::new());
            
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

            // Allow cache query + response to propagate.
            tokio::time::sleep(Duration::from_millis(500)).await;

            // 1) Cached event should arrive first.
            let mut found_cached = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
                    Ok(Some((sub_id, bytes))) if sub_id == "sub1" => {
                        let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
                            .expect("valid WorkerMessage");
                        match wm.type_() {
                            fb::MessageType::ParsedNostrEvent => {
                                found_cached = true;
                                break;
                            }
                            fb::MessageType::Eoce | fb::MessageType::ConnectionStatus => continue,
                            other => panic!("unexpected msg type before cached event: {:?}", other),
                        }
                    }
                    Ok(Some(_)) => continue,
                    _ => continue,
                }
            }
            assert!(found_cached, "Expected cached event at event_sink");

            // 2) EOCE should follow the cached event.
            let mut found_eoce = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
                    Ok(Some((sub_id, bytes))) if sub_id == "sub1" => {
                        let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
                            .expect("valid WorkerMessage");
                        if wm.type_() == fb::MessageType::Eoce {
                            found_eoce = true;
                            break;
                        }
                        // May also see ConnectionStatus here; keep polling.
                        if wm.type_() == fb::MessageType::ConnectionStatus {
                            continue;
                        }
                        panic!("expected EOCE after cached event, got {:?}", wm.type_());
                    }
                    Ok(Some(_)) => continue,
                    _ => continue,
                }
            }
            assert!(found_eoce, "Expected EOCE after cached events");

            // 3) Inject a network event from the relay.
            let net_event_json = common::make_event_json(
                "00000000000000000000000000000000000000000000000000000000000000c2",
                PUBKEY,
                1,
                "network hello",
                1234567890,
                SIGNATURE,
            );
            transport.invoke_message_callback(
                "wss://r1",
                format!(r#"["EVENT","sub1",{}]"#, net_event_json),
            );

            let mut found_network = false;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline {
                match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
                    Ok(Some((sub_id, bytes))) if sub_id == "sub1" => {
                        let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
                            .expect("valid WorkerMessage");
                        match wm.type_() {
                            fb::MessageType::ParsedNostrEvent => {
                                found_network = true;
                                break;
                            }
                            fb::MessageType::ConnectionStatus => continue,
                            other => panic!("unexpected msg type for network event: {:?}", other),
                        }
                    }
                    Ok(Some(_)) => continue,
                    _ => continue,
                }
            }
            assert!(found_network, "Expected network event at event_sink");

            // 4) Verify the cache worker was queried.
            let query_calls = storage.get_query_calls();
            assert!(
                !query_calls.is_empty(),
                "storage should have received at least one query"
            );
        })
        .await;
}
