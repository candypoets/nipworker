pub mod cache_processor;
pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::generated::nostr::fb::{self, WorkerMessage};
use crate::nostr::Template;
use crate::parser::Parser;
use crate::pipeline::{
    CounterPipe, KindFilterPipe, NpubLimiterPipe, ParsePipe, PipeType, ProofVerificationPipe,
    SaveToDbPipe, SerializeEventsPipe,
};
use crate::relays::ClientMessage;
use crate::types::network::Request;
use crate::utils::buffer::SharedBufferManager;
use crate::utils::js_interop::post_worker_message;
use crate::utils::sab_ring::SabRing;
use crate::NostrError;
use crate::{db::NostrDB, pipeline::Pipeline};
use flatbuffers::FlatBufferBuilder;
use futures::lock::Mutex;
use gloo_timers::future::TimeoutFuture;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

type Result<T> = std::result::Result<T, NostrError>;

// Tunables
const MAX_INFLIGHT: usize = 6;
const STARTUP_DELAY_MS: u32 = 500;
const INITIAL_BACKOFF_MS: u32 = 8;
const MAX_BACKOFF_MS: u32 = 512;

struct Sub {
    pipeline: Arc<Mutex<Pipeline>>,
    buffer: SharedArrayBuffer,
    eosed: bool,
    publish_id: Option<String>,
}

pub struct NetworkManager {
    ws_response: Rc<RefCell<SabRing>>,
    cache_response: Rc<RefCell<SabRing>>,
    cache_request: Rc<RefCell<SabRing>>,
    publish_manager: publish::PublishManager,
    subscription_manager: subscription::SubscriptionManager,
    subscriptions: Arc<RwLock<FxHashMap<String, Sub>>>,
}

// Fast, zero-allocation unquote: removes a single pair of "..." if present.
// Assumes no escaped quotes at the ends (which is true for standard JSON tokens).
fn unquote_simple(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && b.first() == Some(&b'"') && b.last() == Some(&b'"') {
        &s[1..b.len() - 1]
    } else {
        s
    }
}

impl NetworkManager {
    pub fn new(
        database: Arc<NostrDB>,
        parser: Arc<Parser>,
        cache_request: Rc<RefCell<SabRing>>,
        cache_response: Rc<RefCell<SabRing>>,
        ws_response: Rc<RefCell<SabRing>>,
    ) -> Self {
        let publish_manager = publish::PublishManager::new(database.clone(), parser.clone());
        let subscription_manager =
            subscription::SubscriptionManager::new(database.clone(), parser.clone());

        let manager = Self {
            ws_response,
            cache_request,
            cache_response,
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
        };

        manager.start_response_reader();
        manager
    }

    // Simplified: Extract prefixed fb_bytes as Arc (root later where needed)
    fn parse_out_envelope(bytes: &[u8]) -> Option<Arc<Vec<u8>>> {
        // Prefix check only
        if bytes.len() < 4 {
            return None;
        }
        let fb_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
        if fb_len == 0 || bytes.len() < 4 + fb_len {
            warn!("Invalid prefix/length in out-ring: len={}", fb_len);
            return None;
        }
        let fb_bytes = &bytes[4..4 + fb_len];
        Some(Arc::new(fb_bytes.to_vec())) // Shared owned fb_bytes
    }

    async fn handle_message_core(
        subs: Arc<RwLock<FxHashMap<String, Sub>>>,
        fb_bytes_arc: Arc<Vec<u8>>,
    ) {
        let wm = match flatbuffers::root::<fb::WorkerMessage>(&fb_bytes_arc) {
            Ok(w) => w,
            Err(_) => {
                warn!("Re-root failed for WorkerMessage (malformed FB)");
                return;
            }
        };

        let sid = wm.sub_id().unwrap_or("");
        if sid.is_empty() {
            warn!("Invalid WorkerMessage: Missing sub_id");
            return;
        }

        // Extract pipeline and buffer with short-lived lock
        let (pipeline_arc, buffer, publish_id) = {
            let guard = match subs.write() {
                Ok(g) => g,
                Err(_) => {
                    warn!("Subscriptions lock poisoned");
                    return;
                }
            };
            let Some(sub) = guard.get(sid) else {
                warn!("Sub not found for {}", sid);
                return;
            };
            (
                Arc::clone(&sub.pipeline),
                sub.buffer.clone(),
                sub.publish_id.clone(),
            )
        };

        match wm.type_() {
            fb::MessageType::ConnectionStatus => {
                let Some(cs) = wm.content_as_connection_status() else {
                    warn!("WorkerMessage ConnectionStatus missing content");
                    return;
                };
                let url = wm.url().unwrap_or("");
                let status = cs.status();
                match status {
                    "NOTICE" => {
                        info!(
                            "Received notice from {:?}: {}",
                            url,
                            cs.message().unwrap_or("")
                        );
                    }
                    "AUTH" => {
                        info!("Auth needed on relay {:?}", url);
                    }
                    "CLOSED" => {
                        info!("Sub closed {}, on relay {:?}", sid, url);
                    }
                    "EOSE" => {
                        // EOSE: signal to UI/pipeline and mark subscription as eosed
                        SharedBufferManager::send_connection_status(&buffer, url, "EOSE", "").await;
                        post_worker_message(&JsValue::from_str(sid));
                        if let Ok(mut w) = subs.write() {
                            if let Some(sub) = w.get_mut(sid) {
                                sub.eosed = true;
                            }
                        }
                    }
                    "OK" => {
                        // OK: forward to UI, and notify any publish waiter
                        let msg = cs.message().unwrap_or("");
                        SharedBufferManager::send_connection_status(&buffer, url, msg, "").await;

                        if let Some(pub_id) = publish_id {
                            post_worker_message(&JsValue::from_str(&pub_id));
                        }
                    }
                    other => {
                        warn!("Unexpected ConnectionStatus '{}' for sub {}", other, sid);
                    }
                }
            }
            fb::MessageType::Raw => {
                // For now, keep the existing pipeline.process(&str) path
                let Some(raw) = wm.content_as_raw() else {
                    warn!("WorkerMessage Raw missing content");
                    return;
                };
                let raw_msg = raw.raw();
                if raw_msg.is_empty() {
                    warn!("Empty Raw message for sub {}", sid);
                    return;
                }

                let mut pipeline_guard = pipeline_arc.lock().await;
                match pipeline_guard.process(raw_msg).await {
                    Ok(Some(output)) => {
                        if let Err(e) = SharedBufferManager::write_to_buffer(&buffer, &output).await
                        {
                            warn!("Buffer write failed for sub {}: {:?}", sid, e);
                        }
                    }
                    Ok(None) => { /* dropped by pipeline */ }
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }

                // Once Pipeline accepts bytes, replace with:
                // match pipeline_guard.process_worker_message(&fb_bytes_arc).await { ... }
            }
            _ => {
                // Ignore other message types in this reader
            }
        }
    }

    fn start_response_reader(&self) {
        use futures::{channel::mpsc, FutureExt, StreamExt};
        use std::cell::Cell;

        let subs = self.subscriptions.clone();

        let inflight: Rc<Cell<usize>> = Rc::new(Cell::new(0));
        let (slot_tx, mut slot_rx) = mpsc::unbounded::<()>();

        let ws_response = self.ws_response.clone();
        let cache_response = self.cache_response.clone();

        spawn_local({
            let inflight = inflight.clone();
            let slot_tx_main = slot_tx.clone();
            async move {
                TimeoutFuture::new(STARTUP_DELAY_MS).await;

                let mut empty_backoff_ms: u32 = INITIAL_BACKOFF_MS;
                let mut full_backoff_ms: u32 = INITIAL_BACKOFF_MS;
                let mut prefer_cache: bool = true; // start with cache priority

                loop {
                    if inflight.get() >= MAX_INFLIGHT {
                        let mut timeout = TimeoutFuture::new(full_backoff_ms).fuse();
                        let mut slot = slot_rx.next().fuse();
                        futures::select! {
                            _ = timeout => full_backoff_ms = (full_backoff_ms.saturating_mul(2)).min(MAX_BACKOFF_MS),
                            _ = slot => full_backoff_ms = INITIAL_BACKOFF_MS,
                        }
                        continue;
                    }

                    // Fair, cache-preferred read: try preferred first, then the other
                    let next_bytes = if prefer_cache {
                        if let Some(bytes) = { cache_response.borrow_mut().read_next() } {
                            prefer_cache = false; // alternate next loop
                            Some(bytes)
                        } else if let Some(bytes) = { ws_response.borrow_mut().read_next() } {
                            prefer_cache = true; // alternate next loop
                            Some(bytes)
                        } else {
                            None
                        }
                    } else {
                        if let Some(bytes) = { ws_response.borrow_mut().read_next() } {
                            prefer_cache = true; // alternate next loop
                            Some(bytes)
                        } else if let Some(bytes) = { cache_response.borrow_mut().read_next() } {
                            prefer_cache = false; // alternate next loop
                            Some(bytes)
                        } else {
                            None
                        }
                    };

                    if let Some(bytes) = next_bytes {
                        empty_backoff_ms = INITIAL_BACKOFF_MS;

                        // Light-root directly from the Vec<u8> for cheap routing
                        let wm = match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                            Ok(w) => w,
                            Err(_) => {
                                warn!("WorkerMessage decode failed in reader; dropping frame");
                                continue;
                            }
                        };

                        let sid = wm.sub_id().unwrap_or("").to_string();
                        if sid.is_empty() {
                            warn!("Invalid message: Missing sub_id");
                            continue;
                        }

                        // Early handling of non-heavy status lines (NOTICE/AUTH/CLOSED)
                        if wm.type_() == fb::MessageType::ConnectionStatus {
                            if let Some(cs) = wm.content_as_connection_status() {
                                match cs.status() {
                                    "NOTICE" => {
                                        info!(
                                            "Received notice from {:?}: {}",
                                            wm.url(),
                                            cs.message().unwrap_or("")
                                        );
                                        continue;
                                    }
                                    "AUTH" => {
                                        info!("Auth needed on relay {:?}", wm.url());
                                        continue;
                                    }
                                    "CLOSED" => {
                                        info!("Sub closed {}, on relay {:?}", sid, wm.url());
                                        continue;
                                    }
                                    _ => { /* fall through for EOSE/OK */ }
                                }
                            } else {
                                continue;
                            }
                        }

                        // For heavy processing (Raw, EOSE, OK), ensure sub exists
                        let has_sub = {
                            match subs.read() {
                                Ok(g) => g.contains_key(&sid),
                                Err(_) => {
                                    warn!("Subscriptions lock poisoned; dropping frame");
                                    false
                                }
                            }
                        };
                        if !has_sub {
                            warn!("Subscription {} not found; dropping frame", sid);
                            continue;
                        }

                        // Move bytes into the task (Arc for cheap clone if needed)
                        let fb_arc = std::sync::Arc::new(bytes);

                        inflight.set(inflight.get() + 1);
                        let inflight_clone = inflight.clone();
                        let subs_clone = subs.clone();
                        let slot_tx = slot_tx_main.clone();

                        spawn_local(async move {
                            Self::handle_message_core(subs_clone, fb_arc).await;

                            inflight_clone.set(inflight_clone.get().saturating_sub(1));
                            let _ = slot_tx.unbounded_send(());
                        });

                        TimeoutFuture::new(0).await;
                        full_backoff_ms = INITIAL_BACKOFF_MS;
                    } else {
                        TimeoutFuture::new(empty_backoff_ms).await;
                        empty_backoff_ms = (empty_backoff_ms.saturating_mul(2)).min(MAX_BACKOFF_MS);
                    }
                }
            }
        });
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        shared_buffer: SharedArrayBuffer,
        requests: &Vec<fb::Request<'_>>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<()> {
        // early bailout if the sub already exist
        if self
            .subscriptions
            .read()
            .map(|g| g.contains_key(&subscription_id))
            .unwrap_or(false)
        {
            return Ok(());
        }

        let parsed_requests: Vec<Request> = requests.iter().map(Request::from_flatbuffer).collect();

        let pipeline = self
            .subscription_manager
            .process_subscription(&subscription_id, parsed_requests, config)
            .await?;

        if let Ok(mut w) = self.subscriptions.write() {
            w.insert(
                subscription_id.clone(),
                Sub {
                    pipeline: Arc::new(Mutex::new(pipeline)),
                    buffer: shared_buffer.clone(),
                    eosed: false,
                    // relay_urls: relay_filters.keys().cloned().collect(),
                    publish_id: None,
                },
            );
        } else {
            warn!(
                "Subscriptions lock poisoned while opening sub {}",
                subscription_id
            );
            return Ok(());
        }

        // Construct and write one REQ frame per relay group:
        // ["REQ", subscription_id, ...filters]
        // for (relay_url, filters) in relay_filters {
        //     let req_message = ClientMessage::req(subscription_id.clone(), filters);
        //     let frame = req_message.to_json()?;
        //     self.send_frame_to_relay(&relay_url, &frame);
        // }

        {
            let mut builder = FlatBufferBuilder::new();

            let sid = builder.create_string(&subscription_id);

            // Convert incoming fb::Request<'_> to offsets using unpack -> RequestT -> pack
            let req_offsets: Vec<_> = requests
                .iter()
                .map(|r| {
                    let rt = r.unpack();
                    rt.pack(&mut builder)
                })
                .collect();

            let req_vec = if req_offsets.is_empty() {
                None
            } else {
                Some(builder.create_vector(&req_offsets))
            };

            let cache_req = fb::CacheRequest::create(
                &mut builder,
                &fb::CacheRequestArgs {
                    sub_id: Some(sid),
                    requests: req_vec,
                },
            );

            builder.finish(cache_req, None);
            let bytes = builder.finished_data().to_vec();

            // Write raw CacheRequest bytes to the cache_request ring
            self.cache_request.borrow_mut().write(&bytes);
        }

        Ok(())
    }

    // deprecated, should be called on connections directly
    pub async fn close_subscription(&self, subscription_id: String) -> Result<()> {
        // if let Ok(g) = self.subscriptions.read() {
        //     if let Some(sub) = g.get(&subscription_id) {
        //         // Write a CLOSE frame to each relay
        //         for relay_url in &sub.relay_urls {
        //             let close_message = ClientMessage::close(subscription_id.clone());
        //             let frame = close_message.to_json()?;
        //             self.send_frame_to_relay(relay_url, &frame);
        //         }
        //     }
        // }

        // // Remove the subscription from the map
        // if let Ok(mut w) = self.subscriptions.write() {
        //     w.remove(&subscription_id);
        // }

        // Ok(())
        Ok(())
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
        default_relays: &Vec<String>,
        shared_buffer: SharedArrayBuffer,
    ) -> Result<()> {
        let (event, relays) = self
            .publish_manager
            .publish_event(publish_id.clone(), template)
            .await?;

        let mut all_relays = relays.clone();

        all_relays.extend(default_relays.iter().cloned());
        all_relays.sort();
        all_relays.dedup();

        if let Ok(mut w) = self.subscriptions.write() {
            w.insert(
                event.id.to_string(),
                Sub {
                    pipeline: Arc::new(Mutex::new(Pipeline::new(vec![], "".to_string()).unwrap())),
                    buffer: shared_buffer.clone(),
                    eosed: false,
                    // relay_urls: all_relays.clone(),
                    publish_id: Some(publish_id.clone()),
                },
            );
        } else {
            warn!(
                "Subscriptions lock poisoned while publishing {}",
                publish_id
            );
        }

        // for relay_url in &all_relays {
        //     let event_message = ClientMessage::event(event.clone());
        //     let frame = event_message.to_json()?;
        //     self.send_frame_to_relay(relay_url, &frame);
        // }

        Ok(())
    }

    // Small helper to avoid repeating envelope writes
    // fn send_frame_to_relay(&self, relay_url: &str, frame: &str) {
    //     let relays = [relay_url];
    //     let frames = [frame.to_owned()];
    //     let _ = self.rings.write_in_envelope(&relays, &frames);
    // }
}
