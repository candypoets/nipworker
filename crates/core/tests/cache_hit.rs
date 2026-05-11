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
const TARGET_ID: &str = "870db1dc9eb056c0791882762065381f0814cc1ba1f89910a465bc8b1f205c9a";

fn event_json_with_tags(
    id: &str,
    pubkey: &str,
    kind: u16,
    tags: Vec<Vec<&str>>,
    content: &str,
    created_at: u64,
    sig: &str,
) -> String {
    serde_json::json!({
        "id": id,
        "pubkey": pubkey,
        "created_at": created_at,
        "kind": kind,
        "tags": tags,
        "content": content,
        "sig": sig,
    })
    .to_string()
}

fn build_counter_subscribe_message(sub_id: &str, requests: Vec<Request>) -> Vec<u8> {
    let mut builder = flatbuffers::FlatBufferBuilder::new();
    let sub_id_offset = builder.create_string(sub_id);
    let request_offsets: Vec<_> = requests
        .iter()
        .map(|request| request.build_flatbuffer(&mut builder))
        .collect();
    let requests_offset = builder.create_vector(&request_offsets);
    let mut counter = fb::CounterPipeConfigT::default();
    counter.kinds = vec![1, 6, 7, 17];
    counter.pubkey = PUBKEY.to_string();

    let mut pipe = fb::PipeT::default();
    pipe.config = fb::PipeConfigT::CounterPipeConfig(Box::new(counter));

    let mut pipeline = fb::PipelineConfigT::default();
    pipeline.pipes = vec![pipe];

    let mut config = fb::SubscriptionConfigT::default();
    config.pipeline = Some(Box::new(pipeline));
    config.close_on_eose = true;
    config.cache_first = true;
    config.timeout_ms = 500;
    config.cache_only = true;
    let config_offset = config.pack(&mut builder);
    let subscribe = fb::Subscribe::create(
        &mut builder,
        &fb::SubscribeArgs {
            subscription_id: Some(sub_id_offset),
            requests: Some(requests_offset),
            config: Some(config_offset),
        },
    );
    let main_message = fb::MainMessage::create(
        &mut builder,
        &fb::MainMessageArgs {
            content_type: fb::MainContent::Subscribe,
            content: Some(subscribe.as_union_value()),
        },
    );
    builder.finish(main_message, None);
    builder.finished_data().to_vec()
}

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

#[tokio::test]
async fn test_cache_only_counter_counts_likes_for_e_tag() {
    let local = LocalSet::new();
    local
        .run_until(async {
            let storage = Arc::new(common::MockStorage::new());
            let transport = Arc::new(common::MockRelayTransport::new());
            let (event_sink_tx, mut event_sink_rx) =
                futures::channel::mpsc::channel::<(String, Vec<u8>)>(100);

            let engine = NostrEngine::new(transport.clone(), storage.clone(), event_sink_tx);

            engine
                .subscribe(
                    "seed-likes".to_string(),
                    vec![Request {
                        kinds: vec![1, 7],
                        relays: vec!["wss://r1".to_string()],
                        ..Default::default()
                    }],
                )
                .await
                .unwrap();

            tokio::time::sleep(Duration::from_millis(100)).await;

            let unrelated_note = event_json_with_tags(
                "0000000000000000000000000000000000000000000000000000000000000101",
                PUBKEY,
                1,
                vec![],
                "unrelated note",
                1234560001,
                SIGNATURE,
            );
            let target_like = event_json_with_tags(
                "0000000000000000000000000000000000000000000000000000000000000701",
                PUBKEY,
                7,
                vec![vec!["e", TARGET_ID]],
                "+",
                1234560002,
                SIGNATURE,
            );
            let other_like = event_json_with_tags(
                "0000000000000000000000000000000000000000000000000000000000000702",
                PUBKEY,
                7,
                vec![vec![
                    "e",
                    "0000000000000000000000000000000000000000000000000000000000000999",
                ]],
                "+",
                1234560003,
                SIGNATURE,
            );

            for event in [&unrelated_note, &target_like, &other_like] {
                transport.invoke_message_callback(
                    "wss://r1",
                    format!(r#"["EVENT","seed-likes",{}]"#, event),
                );
            }

            tokio::time::sleep(Duration::from_millis(500)).await;

            let persisted = storage.get_persisted();
            assert_eq!(persisted.len(), 3, "seed subscription should persist parsed events");
            let target_like_bytes = persisted
                .iter()
                .find(|bytes| {
                    let wm = flatbuffers::root::<fb::WorkerMessage>(bytes).unwrap();
                    wm.content_as_parsed_event()
                        .map(|event| event.id() == "0000000000000000000000000000000000000000000000000000000000000701")
                        .unwrap_or(false)
                })
                .expect("target like should be persisted")
                .clone();

            while let Ok(Some(_)) = event_sink_rx.try_next() {}

            storage.set_query_results(vec![vec![], vec![target_like_bytes]]);

            let mut tags = rustc_hash::FxHashMap::default();
            tags.insert("#e".to_string(), vec![TARGET_ID.to_string()]);
            let subscribe = build_counter_subscribe_message(
                "count-likes",
                vec![
                    Request {
                        kinds: vec![1],
                        tags: tags.clone(),
                        limit: Some(500),
                        relays: vec!["wss://r1".to_string()],
                        cache_first: true,
                        cache_only: true,
                        ..Default::default()
                    },
                    Request {
                        kinds: vec![6, 7, 17],
                        tags,
                        limit: Some(500),
                        relays: vec!["wss://r1".to_string()],
                        cache_first: true,
                        cache_only: true,
                        ..Default::default()
                    },
                ],
            );
            engine.handle_message(&subscribe).await.unwrap();

            let mut counts_by_kind = std::collections::HashMap::new();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
            while tokio::time::Instant::now() < deadline && counts_by_kind.len() < 4 {
                match tokio::time::timeout(Duration::from_millis(200), event_sink_rx.next()).await {
                    Ok(Some((sub_id, bytes))) if sub_id == "count-likes" => {
                        let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes)
                            .expect("valid WorkerMessage");
                        if wm.content_type() == fb::Message::CountResponse {
                            let count = wm.content_as_count_response().unwrap();
                            counts_by_kind.insert(count.kind(), count.count());
                        }
                    }
                    Ok(Some(_)) => continue,
                    _ => continue,
                }
            }

            assert_eq!(counts_by_kind.get(&1), Some(&0));
            assert_eq!(counts_by_kind.get(&6), Some(&0));
            assert_eq!(counts_by_kind.get(&7), Some(&1));
            assert_eq!(counts_by_kind.get(&17), Some(&0));

            let query_calls = storage.get_query_calls();
            assert_eq!(query_calls.len(), 3, "seed query plus two counter cache queries");
            assert_eq!(
                query_calls[1][0].e_tags.as_deref(),
                Some(&[TARGET_ID.to_string()][..])
            );
            assert_eq!(
                query_calls[2][0].e_tags.as_deref(),
                Some(&[TARGET_ID.to_string()][..])
            );
        })
        .await;
}
