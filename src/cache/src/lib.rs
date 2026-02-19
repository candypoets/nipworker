#![allow(async_fn_in_trait)]

use crate::db::NostrDB;
use crate::utils::wrap_event_with_worker_message;
use flatbuffers::FlatBufferBuilder;
use futures::channel::mpsc;
use futures::SinkExt;
use futures::StreamExt;
use serde_json::{Map, Value};
use shared::generated::nostr::fb;
use shared::{init_with_component, Port};
use tracing::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::MessagePort;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

const MAX_CONCURRENT_QUERIES: usize = 10;

mod db;
mod utils;

#[wasm_bindgen]
pub struct Caching {
    /// Port to send messages to connections worker
    to_connections: Rc<RefCell<Port>>,
    /// Port to send messages back to parser worker
    to_parser: Rc<RefCell<Port>>,
    /// Database instance
    database: Arc<NostrDB>,
}

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.snort.social",
    "wss://relay.damus.io",
    "wss://relay.primal.net",
];
const INDEXER_RELAYS: &[&str] = &[
    "wss://user.kindpag.es",
    "wss://relay.nos.social",
    "wss://purplepag.es",
    "wss://relay.nostr.band",
];

#[wasm_bindgen]
impl Caching {
    #[wasm_bindgen(constructor)]
    pub async fn new(
        max_buffer_size: usize,
        from_parser: MessagePort,
        to_connections: MessagePort,
    ) -> Self {
        init_with_component(tracing::Level::ERROR, "CACHE");

        info!("instanciating cache");

        let to_connections = Rc::new(RefCell::new(Port::new(to_connections)));
        let from_parser_rx = Port::from_receiver(from_parser.clone());
        let to_parser = Rc::new(RefCell::new(Port::new(from_parser)));

        let database = Arc::new(NostrDB::new(
            "nostr".to_string(),
            max_buffer_size,
            DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect(),
            INDEXER_RELAYS.iter().map(|s| s.to_string()).collect(),
        ));

        database
            .initialize()
            .await
            .map_err(|e| e)
            .expect("Database initialization failed");

        let caching = Caching {
            to_connections,
            to_parser,
            database,
        };

        caching.start(from_parser_rx);

        caching
    }

    fn fb_request_to_req_filter_json(fb_req: &fb::Request<'_>) -> serde_json::Value {
        let mut filter = Map::new();

        if let Some(ids) = fb_req.ids() {
            let arr: Vec<_> = (0..ids.len())
                .map(|i| Value::String(ids.get(i).to_string()))
                .collect();
            if !arr.is_empty() {
                filter.insert("ids".to_string(), Value::Array(arr));
            }
        }
        if let Some(authors) = fb_req.authors() {
            let arr: Vec<_> = (0..authors.len())
                .map(|i| Value::String(authors.get(i).to_string()))
                .collect();
            if !arr.is_empty() {
                filter.insert("authors".to_string(), Value::Array(arr));
            }
        }
        if let Some(kinds) = fb_req.kinds() {
            let arr: Vec<_> = kinds
                .into_iter()
                .map(|k| Value::Number((k as i64).into()))
                .collect();
            if !arr.is_empty() {
                filter.insert("kinds".to_string(), Value::Array(arr));
            }
        }
        if let Some(s) = fb_req.search() {
            if !s.is_empty() {
                filter.insert("search".to_string(), Value::String(s.to_string()));
            }
        }
        let limit = fb_req.limit();
        if limit > 0 {
            filter.insert("limit".to_string(), Value::Number((limit as i64).into()));
        }
        let since = fb_req.since();
        if since > 0 {
            filter.insert("since".to_string(), Value::Number((since as i64).into()));
        }
        let until = fb_req.until();
        if until > 0 {
            filter.insert("until".to_string(), Value::Number((until as i64).into()));
        }

        // Tags: mirror Request::from_flatbuffer
        if let Some(tags) = fb_req.tags() {
            for j in 0..tags.len() {
                let t = tags.get(j);
                if let Some(items) = t.items() {
                    if items.len() >= 2 {
                        let key = items.get(0).to_string(); // keep exactly as provided
                        let vals: Vec<_> = (1..items.len())
                            .map(|k| Value::String(items.get(k).to_string()))
                            .collect();
                        filter.insert(key, Value::Array(vals));
                    }
                }
            }
        }

        Value::Object(filter)
    }

    async fn process_cache_request(
        database: &Arc<NostrDB>,
        to_connections: &Rc<RefCell<Port>>,
        to_parser: &Rc<RefCell<Port>>,
        bytes: &[u8],
    ) {
        let cache_req = match flatbuffers::root::<fb::CacheRequest>(bytes) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to decode CacheRequest: {:?}", e);
                return;
            }
        };

        let sub_id = cache_req.sub_id().to_string();

        // Check if this is an event persist request (event field is set)
        if let Some(fb_event) = cache_req.event() {
            // Persist event and optionally publish
            Self::handle_event_persist(database, to_connections, &sub_id, fb_event, cache_req.relays()).await;
            return;
        }

        // Otherwise, this is a lookup request (requests field should be set)
        if let Some(vec) = cache_req.requests() {
            Self::handle_lookup_request(database, to_connections, to_parser, &sub_id, cache_req.requests()).await;
        }
    }

    async fn handle_event_persist(
        database: &Arc<NostrDB>,
        to_connections: &Rc<RefCell<Port>>,
        sub_id: &str,
        fb_event: fb::NostrEvent<'_>,
        relay_hints: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<&'_ str>>>,
    ) {
        // First, persist the event to database
        if let Err(e) = database.add_event_from_fb(fb_event).await {
            warn!("Failed to persist event to database: {}", e);
        }

        // Determine relays to publish to
        let mut relays: Vec<String> = database
            .determine_target_relays(fb_event)
            .await
            .unwrap_or_default();

        if let Some(rs) = relay_hints {
            for i in 0..rs.len() as usize {
                relays.push(rs.get(i).to_string());
            }
        }

        // Fallback to DEFAULT_RELAYS if none were determined
        if relays.is_empty() {
            warn!(
                "[PUBLISH] No relays determined for event {} - using defaults",
                fb_event.id()
            );
            relays = DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect();
        }

        // Deduplicate relays
        relays.sort();
        relays.dedup();

        // Build Nostr event JSON from flatbuffer
        let tags_vec = fb_event.tags();
        let mut tags_json: Vec<serde_json::Value> = Vec::with_capacity(tags_vec.len());
        for i in 0..tags_vec.len() {
            let sv = tags_vec.get(i);
            if let Some(items) = sv.items() {
                let arr: Vec<serde_json::Value> = (0..items.len())
                    .map(|j| serde_json::Value::String(items.get(j).to_string()))
                    .collect();
                tags_json.push(serde_json::Value::Array(arr));
            } else {
                tags_json.push(serde_json::Value::Array(vec![]));
            }
        }

        let event_json = serde_json::json!({
            "id": fb_event.id(),
            "pubkey": fb_event.pubkey(),
            "kind": fb_event.kind(),
            "content": fb_event.content(),
            "tags": tags_json,
            "created_at": fb_event.created_at(),
            "sig": fb_event.sig(),
        });

        // Frame: ["EVENT", event]
        let frame_val = serde_json::Value::Array(vec![
            serde_json::Value::String("EVENT".to_string()),
            event_json,
        ]);

        let frame_str = serde_json::to_string(&frame_val).unwrap_or_else(|_| "[]".to_string());

        let env = serde_json::json!({
            "relays": relays,
            "frames": [frame_str],
        });

        let env_str = env.to_string();

        info!("Publishing event to relays: {:?}", relays);

        // Send through to_connections port
        if let Err(e) = to_connections.borrow().send(env_str.as_bytes()) {
            warn!("Failed to send publish frame to connections: {:?}", e);
        }
    }

    async fn handle_lookup_request(
        database: &Arc<NostrDB>,
        to_connections: &Rc<RefCell<Port>>,
        to_parser: &Rc<RefCell<Port>>,
        sub_id: &str,
        requests: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<fb::Request<'_>>>>,
    ) {
        // Use DB helper to get cached events + indices to forward
        let (remaining_idxs, cached_events) = match database
            .query_events_and_requests(requests)
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                warn!("query_events_and_requests failed: {}", e);
                (Vec::new(), Vec::new())
            }
        };

        if !cached_events.is_empty() {
            info!(
                "Found {} cached events for subscription {}",
                cached_events.len(),
                sub_id
            );
        }

        // 1) Send cached WorkerMessage bytes directly to parser
        for ev_bytes in cached_events {
            if let Some(wm) = wrap_event_with_worker_message(sub_id, &ev_bytes) {
                if let Err(e) = to_parser.borrow().send(&wm) {
                    warn!("Failed to send cached event to parser: {:?}", e);
                }
            } else {
                warn!("Failed to wrap cached event into WorkerMessage");
            }
        }

        // 2) Send remaining requests as REQ frames to connections
        if let Some(vec) = requests {
            info!(
                "sending {}/{} requests for {}",
                remaining_idxs.len(),
                vec.len(),
                sub_id
            );
            for idx in remaining_idxs {
                let fb_req = vec.get(idx);

                // relays: prefer request.relays; else get from kind 10002 events or appropriate fallback
                let relays: Vec<String> = if let Some(rs) = fb_req.relays() {
                    let relay_list: Vec<String> =
                        (0..rs.len()).map(|i| rs.get(i).to_string()).collect();
                    if relay_list.is_empty() {
                        // Get relays from kind 10002 events or appropriate fallback
                        database.get_relays(&fb_req)
                    } else {
                        relay_list
                    }
                } else {
                    // Get relays from kind 10002 events or appropriate fallback
                    database.get_relays(&fb_req)
                };

                info!("Using relays for {}: {:?}", sub_id, relays);

                // Build filter JSON using the same tag mapping as Request::from_flatbuffer
                let filter_json = Caching::fb_request_to_req_filter_json(&fb_req);

                // Frame: ["REQ", sub_id, filter]
                let frame_val = serde_json::Value::Array(vec![
                    serde_json::Value::String("REQ".to_string()),
                    serde_json::Value::String(sub_id.to_string()),
                    filter_json,
                ]);
                let frame_str =
                    serde_json::to_string(&frame_val).unwrap_or_else(|_| "[]".to_string());

                // Envelope: { relays: [...], frames: [ "<frame>" ] }
                let env = serde_json::json!({
                    "relays": relays,
                    "frames": [frame_str],
                });

                let env_str = env.to_string();

                if let Err(e) = to_connections.borrow().send(env_str.as_bytes()) {
                    warn!("Failed to send REQ frame to connections: {:?}", e);
                }
            }

            // 3) Emit EOCE through to_connections port
            let mut builder = FlatBufferBuilder::new();
            let sid = builder.create_string(sub_id);
            let eoce = fb::Eoce::create(
                &mut builder,
                &fb::EoceArgs {
                    subscription_id: Some(sid),
                },
            );
            let msg = fb::WorkerMessage::create(
                &mut builder,
                &fb::WorkerMessageArgs {
                    sub_id: Some(sid),
                    url: None,
                    type_: fb::MessageType::Eoce,
                    content_type: fb::Message::Eoce,
                    content: Some(eoce.as_union_value()),
                },
            );
            builder.finish(msg, None);
            let eoce_bytes = builder.finished_data().to_vec();

            if let Err(e) = to_connections.borrow().send(&eoce_bytes) {
                warn!("Failed to send EOCE: {:?}", e);
            }
        }
    }

    /// Check if request is an event persist (has event field set)
    fn is_persist_request(bytes: &[u8]) -> bool {
        match flatbuffers::root::<fb::CacheRequest>(bytes) {
            Ok(req) => req.event().is_some(),
            Err(_) => false,
        }
    }

    /// Start cache workers: 1 single-threaded writer for event persistence,
    /// plus a semaphore pool for concurrent queries
    fn start(&self, mut from_parser_rx: mpsc::Receiver<Vec<u8>>) {
        info!(
            "starting cache: 1 writer thread + {} query workers",
            MAX_CONCURRENT_QUERIES
        );

        let to_connections = self.to_connections.clone();
        let to_parser = self.to_parser.clone();
        let database = self.database.clone();

        // Channel for routing persist requests to single writer
        let (writer_tx, mut writer_rx) = mpsc::channel::<Vec<u8>>(128);

        // Semaphore for limiting concurrent query operations
        let query_semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_QUERIES));

        // === SINGLE WRITER THREAD (Event Persistence) ===
        // Event writes must be sequential to avoid IndexedDB transaction conflicts
        let db_writer = database.clone();
        let tc_writer = to_connections.clone();
        let tp_writer = to_parser.clone();
        spawn_local(async move {
            loop {
                match writer_rx.next().await {
                    Some(bytes) => {
                        Caching::process_cache_request(&db_writer, &tc_writer, &tp_writer, &bytes)
                            .await;
                    }
                    None => {
                        info!("Cache writer channel closed, exiting");
                        break;
                    }
                }
            }
        });

        // === DISTRIBUTOR: Route to writer or query pool ===
        spawn_local(async move {
            loop {
                match from_parser_rx.next().await {
                    Some(bytes) => {
                        if Caching::is_persist_request(&bytes) {
                            // Persist: route to single writer thread
                            if let Err(e) = writer_tx.clone().send(bytes).await {
                                warn!("Failed to send to writer: {:?}", e);
                            }
                        } else {
                            // Query: acquire semaphore permit and process concurrently
                            let permit = match query_semaphore.clone().try_acquire_owned() {
                                Ok(p) => p,
                                Err(_) => {
                                    // If at capacity, wait for a permit
                                    match query_semaphore.clone().acquire_owned().await {
                                        Ok(p) => p,
                                        Err(e) => {
                                            warn!("Semaphore closed: {:?}", e);
                                            continue;
                                        }
                                    }
                                }
                            };

                            let db = database.clone();
                            let tc = to_connections.clone();
                            let tp = to_parser.clone();

                            spawn_local(async move {
                                let _permit = permit; // Hold until done
                                Caching::process_cache_request(&db, &tc, &tp, &bytes).await;
                            });
                        }
                    }
                    None => {
                        info!("Cache distributor channel closed, exiting");
                        break;
                    }
                }
            }
        });
    }
}
