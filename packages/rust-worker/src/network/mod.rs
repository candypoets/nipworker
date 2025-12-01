pub mod cache_processor;
pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::generated::nostr::fb::{self, OutEnvelope};
use crate::nostr::Template;
use crate::parser::Parser;
use crate::pipeline::PipelineEvent;
use crate::relays::ClientMessage;
use crate::types::network::Request;
use crate::utils::buffer::SharedBufferManager;
use crate::utils::js_interop::post_worker_message;
use crate::utils::json::extract_first_three;
use crate::utils::sab_ring::WsRings;
use crate::NostrError;
use crate::{db::NostrDB, pipeline::Pipeline};
use flatbuffers::{root, Verifier};
use futures::lock::Mutex;
use gloo_timers::future::TimeoutFuture;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;
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
    relay_urls: Vec<String>,
    eosed: bool,
    publish_id: Option<String>,
}

pub struct NetworkManager {
    rings: Rc<WsRings>,
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
    pub fn new(database: Arc<NostrDB>, parser: Arc<Parser>, rings: WsRings) -> Self {
        let publish_manager = publish::PublishManager::new(database.clone(), parser.clone());
        let subscription_manager =
            subscription::SubscriptionManager::new(database.clone(), parser.clone());

        let manager = Self {
            rings: Rc::new(rings),
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
        };

        manager.start_out_ring_reader();
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
        sid: &str,
        kind: &str,
        is_eose: bool,
    ) {
        // Re-root OutEnvelope from Arc (direct access)
        // Safety: verify first to avoid UB from malformed buffers
        {
            let mut verifier = Verifier::new(&fb_bytes_arc, 64);
            if !OutEnvelope::verify(&mut verifier, 0) {
                warn!("OutEnvelope verification failed for sub {}", sid);
                return;
            }
        }
        let env = match flatbuffers::root::<OutEnvelope>(&fb_bytes_arc) {
            Ok(e) => e,
            Err(_) => {
                warn!("Re-root failed for sub {} (malformed FB)", sid);
                return;
            }
        };

        // Safety check
        if env.sub_id() != sid {
            warn!("sub_id mismatch in envelope for {}", sid);
            return;
        }

        // Extract pipeline and buffer with a short-lived write lock (no await while locked)
        let (pipeline_arc, buffer) = {
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
            (Arc::clone(&sub.pipeline), sub.buffer.clone())
        };

        // After this point, no RwLock guard is held.

        match kind {
            "EVENT" => {
                // For now, do not process structured cache events here.
                // You will update the pipeline to handle these later.
                if env.parsed_event().is_some() || env.event().is_some() {
                    warn!(
                        "Structured event from cache for sub {} - pipeline update needed; skipping for now",
                        sid
                    );
                    return;
                }

                // Fallback: Raw message from ws-rust (forward &str directly to pipeline.process)
                let raw_msg = env.message().unwrap_or("");
                if raw_msg.is_empty() {
                    warn!("No event data for sub {}", sid);
                    return;
                }

                // Lock pipeline and call process(&str)
                let mut pipeline_guard = pipeline_arc.lock().await;
                match pipeline_guard.process(raw_msg).await {
                    Ok(Some(output)) => {
                        if let Err(e) = SharedBufferManager::write_to_buffer(&buffer, &output).await
                        {
                            warn!("Buffer write failed for sub {}: {:?}", sid, e);
                        }
                    }
                    Ok(None) => {
                        // Dropped by pipeline (e.g., dedup or invalid)
                        // keep silent or debug-level
                    }
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }
            }
            "EOSE" => {
                if is_eose || env.is_eose() {
                    SharedBufferManager::send_eoce(&buffer).await;
                    post_worker_message(&JsValue::from_str(sid));
                }
            }
            _ => {
                warn!("Unexpected kind {} in core for sub {}", kind, sid);
            }
        }
    }

    fn start_out_ring_reader(&self) {
        use futures::{channel::mpsc, FutureExt, StreamExt};
        use std::cell::Cell;

        let rings = self.rings.clone();
        let subs = self.subscriptions.clone();

        let inflight: Rc<Cell<usize>> = Rc::new(Cell::new(0));
        let (slot_tx, mut slot_rx) = mpsc::unbounded::<()>();

        spawn_local({
            let inflight = inflight.clone();
            let slot_tx_main = slot_tx.clone();
            async move {
                TimeoutFuture::new(STARTUP_DELAY_MS).await;

                let mut empty_backoff_ms: u32 = INITIAL_BACKOFF_MS;
                let mut full_backoff_ms: u32 = INITIAL_BACKOFF_MS;

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

                    if let Some(bytes) = rings.read_out() {
                        empty_backoff_ms = INITIAL_BACKOFF_MS;

                        if let Some(fb_arc) = Self::parse_out_envelope(&bytes) {
                            // Verify before root to avoid panics on bad buffers
                            let mut verifier = Verifier::new(&fb_arc, 64);
                            if !OutEnvelope::verify(&mut verifier, 0) {
                                warn!("OutEnvelope verification failed in reader; dropping frame");
                                continue;
                            }

                            // Root in drainer for light filtering (zero-copy)
                            let env = match flatbuffers::root::<OutEnvelope>(&fb_arc) {
                                Ok(e) => e,
                                Err(_) => {
                                    warn!("Re-root failed for envelope in reader; dropping frame");
                                    continue; // keep the reader alive
                                }
                            };

                            // Validate sub_id here (light check)
                            let sid = env.sub_id().to_string();
                            if sid.is_empty() {
                                warn!("Invalid envelope: Missing sub_id");
                                continue;
                            }

                            let kind_str = env.kind().to_string();

                            // Inline light handling (direct accessors)
                            match kind_str.as_str() {
                                "NOTICE" => {
                                    info!(
                                        "Received notice from {:?}: {}",
                                        env.url(),
                                        env.message().unwrap_or("")
                                    );
                                    continue;
                                }
                                "AUTH" => {
                                    info!("Auth needed on relay {:?}", env.url());
                                    continue;
                                }
                                "CLOSED" => {
                                    info!("Sub closed {}, on relay {:?}", sid, env.url());
                                    continue;
                                }
                                "OK" => {
                                    if env.success() {
                                        info!("Publish OK for sub {}", sid);
                                    } else {
                                        warn!("Publish failed for sub {}", sid);
                                    }
                                    continue;
                                }
                                // Heavy: EVENT/EOSE
                                "EVENT" | "EOSE" => {
                                    let has_sub = {
                                        match subs.read() {
                                            Ok(g) => g.contains_key(&sid),
                                            Err(_) => {
                                                warn!(
                                                    "Subscriptions lock poisoned; dropping frame"
                                                );
                                                false
                                            }
                                        }
                                    };
                                    if !has_sub {
                                        continue;
                                    }
                                    // Fall through to spawn
                                }
                                _ => continue,
                            }

                            // Spawn for EVENT/EOSE (pass Arc + extracted)
                            inflight.set(inflight.get() + 1);
                            let inflight_clone = inflight.clone();
                            let subs_clone = subs.clone();
                            let slot_tx = slot_tx_main.clone();
                            let fb_arc_clone = fb_arc.clone(); // Cheap pointer
                            let sid_clone = sid.clone();
                            let kind_clone = kind_str.clone();
                            let is_eose = env.is_eose();

                            spawn_local(async move {
                                Self::handle_message_core(
                                    subs_clone,
                                    fb_arc_clone,
                                    &sid_clone,
                                    &kind_clone,
                                    is_eose,
                                )
                                .await;

                                inflight_clone.set(inflight_clone.get().saturating_sub(1));
                                let _ = slot_tx.unbounded_send(());
                            });

                            TimeoutFuture::new(0).await;
                            full_backoff_ms = INITIAL_BACKOFF_MS;
                        } else {
                            TimeoutFuture::new(0).await; // Malformed prefix
                        }
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

        let (pipeline, relay_filters) = self
            .subscription_manager
            .process_subscription(
                &subscription_id,
                shared_buffer.clone(),
                parsed_requests,
                config,
            )
            .await?;

        if let Ok(mut w) = self.subscriptions.write() {
            w.insert(
                subscription_id.clone(),
                Sub {
                    pipeline: Arc::new(Mutex::new(pipeline)),
                    buffer: shared_buffer.clone(),
                    eosed: false,
                    relay_urls: relay_filters.keys().cloned().collect(),
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
        for (relay_url, filters) in relay_filters {
            let req_message = ClientMessage::req(subscription_id.clone(), filters);
            let frame = req_message.to_json()?;
            self.send_frame_to_relay(&relay_url, &frame);
        }

        Ok(())
    }

    pub async fn close_subscription(&self, subscription_id: String) -> Result<()> {
        if let Ok(g) = self.subscriptions.read() {
            if let Some(sub) = g.get(&subscription_id) {
                // Write a CLOSE frame to each relay
                for relay_url in &sub.relay_urls {
                    let close_message = ClientMessage::close(subscription_id.clone());
                    let frame = close_message.to_json()?;
                    self.send_frame_to_relay(relay_url, &frame);
                }
            }
        }

        // Remove the subscription from the map
        if let Ok(mut w) = self.subscriptions.write() {
            w.remove(&subscription_id);
        }

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
                    relay_urls: all_relays.clone(),
                    publish_id: Some(publish_id.clone()),
                },
            );
        } else {
            warn!(
                "Subscriptions lock poisoned while publishing {}",
                publish_id
            );
        }

        for relay_url in &all_relays {
            let event_message = ClientMessage::event(event.clone());
            let frame = event_message.to_json()?;
            self.send_frame_to_relay(relay_url, &frame);
        }

        Ok(())
    }

    // Small helper to avoid repeating envelope writes
    fn send_frame_to_relay(&self, relay_url: &str, frame: &str) {
        let relays = [relay_url];
        let frames = [frame.to_owned()];
        let _ = self.rings.write_in_envelope(&relays, &frames);
    }
}
