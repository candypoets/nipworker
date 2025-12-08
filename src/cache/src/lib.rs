#![allow(async_fn_in_trait)]

use crate::db::NostrDB;
use crate::generated::nostr::fb;
use crate::utils::wrap_event_with_worker_message;
use flatbuffers::FlatBufferBuilder;
use gloo_timers::future::TimeoutFuture;
use serde_json::{Map, Value};
use shared::{init_with_component, SabRing};
use tracing::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use js_sys::SharedArrayBuffer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};

mod db;
mod generated;
mod utils;

#[wasm_bindgen]
pub struct Caching {
    // Own the rings behind Rc so tasks can hold them without borrowing `self`.
    cache_request: Rc<RefCell<SabRing>>,
    cache_response: Rc<RefCell<SabRing>>,
    ws_request: Rc<RefCell<SabRing>>,
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
        db_ring: SharedArrayBuffer,
        cache_request: SharedArrayBuffer,
        cache_response: SharedArrayBuffer,
        ws_request: SharedArrayBuffer,
    ) -> Self {
        init_with_component(tracing::Level::ERROR, "CACHE");

        info!("instanciating cache");

        let cache_request = Rc::new(RefCell::new(
            SabRing::new(cache_request).expect("Failed to create SabRing for cache_request"),
        ));
        let cache_response = Rc::new(RefCell::new(
            SabRing::new(cache_response).expect("Failed to create SabRing for cache_response"),
        ));
        let ws_request = Rc::new(RefCell::new(
            SabRing::new(ws_request).expect("Failed to create SabRing for ws_request"),
        ));

        let database = Arc::new(NostrDB::new_with_ringbuffer(
            "nostr".to_string(),
            max_buffer_size,
            db_ring,
            DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect(),
            INDEXER_RELAYS.iter().map(|s| s.to_string()).collect(),
        ));

        database
            .initialize()
            .await
            .map_err(|e| e)
            .expect("Database initialization failed");

        let caching = Caching {
            cache_request,
            cache_response,
            ws_request,
            database,
        };

        caching.start();

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

    async fn process_local_requests(
        database: &Arc<NostrDB>,
        cache_response: &Rc<RefCell<SabRing>>,
        ws_request: &Rc<RefCell<SabRing>>,
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

        // Use DB helper to get cached events + indices to forward
        let (remaining_idxs, cached_events) = match database
            .query_events_and_requests(cache_req.requests())
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

        // 1) Write cached WorkerMessage bytes to cache_response
        {
            let mut out = cache_response.borrow_mut();
            for ev_bytes in cached_events {
                if let Some(wm) = wrap_event_with_worker_message(&sub_id, &ev_bytes) {
                    out.write(&wm);
                } else {
                    warn!("Failed to wrap cached event into WorkerMessage");
                }
            }
        }

        // 2) Write remaining requests as REQ frames to ws_request
        if let Some(vec) = cache_req.requests() {
            let mut ws = ws_request.borrow_mut();
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
                    serde_json::Value::String(sub_id.clone()),
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

                ws.write(env_str.as_bytes());
            }
        }

        // 3) Emit EOCE to cache_response
        let mut builder = FlatBufferBuilder::new();
        let sid = builder.create_string(&sub_id);
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

        {
            let mut out = cache_response.borrow_mut();
            let written = out.write(&eoce_bytes);
        }
    }

    /// Start multiple worker loops to process cache requests concurrently
    fn start(&self) {
        info!("starting cache");

        const NUM_WORKERS: usize = 10;

        for _ in 0..NUM_WORKERS {
            // Clone the handles we need into the task
            let cache_request = self.cache_request.clone();
            let cache_response = self.cache_response.clone();
            let ws_request = self.ws_request.clone();
            let database = self.database.clone();

            spawn_local(async move {
                let mut sleep_ms: u32 = 16;
                let max_sleep_ms: u32 = 500;

                loop {
                    let mut processed = 0usize;

                    // Drain ring
                    loop {
                        let bytes_opt = { cache_request.borrow_mut().read_next() };
                        let Some(bytes) = bytes_opt else { break };
                        processed += 1;
                        Caching::process_local_requests(
                            &database,
                            &cache_response,
                            &ws_request,
                            &bytes,
                        )
                        .await;
                    }

                    if processed == 0 {
                        TimeoutFuture::new(sleep_ms).await;
                        sleep_ms = (sleep_ms.saturating_mul(2)).min(max_sleep_ms);
                    } else {
                        sleep_ms = 16;
                    }
                }
            });
        }
    }
}
