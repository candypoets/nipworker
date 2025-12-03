use crate::db::NostrDB;
use crate::generated::nostr::fb;
use flatbuffers::FlatBufferBuilder;
use gloo_timers::future::TimeoutFuture;
use serde_json::{Map, Value};
use tracing::{info, warn};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use js_sys::SharedArrayBuffer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Once};

mod db;
mod generated;
mod sab_ring;

use sab_ring::SabRing;

// Common macros
#[macro_export]
macro_rules! console_log {
    ($($t:tt)*) => {
        web_sys::console::log_1(&format_args!($($t)*).to_string().into());
    }
}

static TRACING_INIT: Once = Once::new();

fn setup_tracing() {
    TRACING_INIT.call_once(|| {
        // Simple console writer for Web Workers
        struct ConsoleWriter;

        impl std::io::Write for ConsoleWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let message = String::from_utf8_lossy(buf);
                web_sys::console::log_1(&JsValue::from_str(&message));
                Ok(buf.len())
            }

            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        // Try to set up a simple subscriber - if it fails, just continue
        let _ = tracing_subscriber::fmt()
            .with_writer(|| ConsoleWriter)
            .without_time()
            .with_target(false)
            .with_max_level(tracing::Level::INFO)
            .try_init();

        console_log!("Tracing subscriber initialized for Web Worker");
    });
}

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
    /// new(inRings: SharedArrayBuffer[], outRings: SharedArrayBuffer[])
    #[wasm_bindgen(constructor)]
    pub fn new(
        max_buffer_size: usize,
        ingest_ring: SharedArrayBuffer,
        cache_request: SharedArrayBuffer,
        cache_response: SharedArrayBuffer,
        ws_request: SharedArrayBuffer,
    ) -> Result<Caching, JsValue> {
        setup_tracing();

        let cache_request = Rc::new(RefCell::new(SabRing::new(cache_request)?));
        let cache_response = Rc::new(RefCell::new(SabRing::new(cache_response)?));
        let ws_request = Rc::new(RefCell::new(SabRing::new(ws_request)?));

        let database = Arc::new(NostrDB::new_with_ringbuffer(
            "nostr".to_string(),
            max_buffer_size,
            ingest_ring,
            DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect(),
            INDEXER_RELAYS.iter().map(|s| s.to_string()).collect(),
        ));

        let db_clone = database.clone();
        spawn_local(async move {
            if let Err(e) = db_clone.initialize().await {
                warn!("Cache DB initialize failed: {}", e);
            } else {
                info!("Cache DB initialized");
            }
        });

        Ok(Caching {
            cache_request,
            cache_response,
            ws_request,
            database,
        })
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

    async fn process_local_requests(&self, bytes: &[u8]) {
        let cache_req = match flatbuffers::root::<fb::CacheRequest>(bytes) {
            Ok(r) => r,
            Err(e) => {
                warn!("Failed to decode CacheRequest: {:?}", e);
                return;
            }
        };

        let sub_id = cache_req.sub_id().to_string();

        // Use DB helper to get cached events + indices to forward
        let (remaining_idxs, cached_events) = match self
            .database
            .query_events_and_requests(cache_req.requests())
            .await
        {
            Ok(pair) => pair,
            Err(e) => {
                warn!("query_events_and_requests failed: {}", e);
                (Vec::new(), Vec::new())
            }
        };

        // 1) Write cached WorkerMessage bytes to cache_response
        {
            let mut out = self.cache_response.borrow_mut();
            for ev_bytes in cached_events {
                out.write(&ev_bytes);
            }
        }

        // 2) Write remaining requests as REQ frames to ws_request
        if let Some(vec) = cache_req.requests() {
            for idx in remaining_idxs {
                let fb_req = vec.get(idx);

                // relays: prefer request.relays; else default list
                let relays: Vec<String> = if let Some(rs) = fb_req.relays() {
                    (0..rs.len()).map(|i| rs.get(i).to_string()).collect()
                } else {
                    DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect()
                };

                // Build filter JSON using the same tag mapping as Request::from_flatbuffer
                let filter_json = Self::fb_request_to_req_filter_json(&fb_req);

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

                self.ws_request
                    .borrow_mut()
                    .write(env.to_string().as_bytes());
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
                type_: fb::MessageType::Eoce,
                content_type: fb::Message::Eoce,
                content: Some(eoce.as_union_value()),
            },
        );
        builder.finish(msg, None);
        let eoce_bytes = builder.finished_data().to_vec();

        let mut out = self.cache_response.borrow_mut();
        out.write(&eoce_bytes);
    }

    /// Start one loop per inRing that reads JSON envelopes and calls send_to_relays
    pub fn start(&self) {
        let ring_rc = self.cache_request.clone();
        let this = self as *const Caching;

        spawn_local(async move {
            // SAFETY: captured self pointer is only used to call an immutable method
            let runner = unsafe { &*this };
            let mut sleep_ms: u32 = 16;
            let max_sleep_ms: u32 = 500;

            loop {
                let mut processed = 0usize;

                // Drain ring
                loop {
                    let bytes_opt = { ring_rc.borrow_mut().read_next() };
                    let Some(bytes) = bytes_opt else { break };
                    processed += 1;
                    runner.process_local_requests(&bytes).await;
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
