pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::crypto_client::CryptoClient;
use crate::parser::Parser;
use crate::pipeline::Pipeline;
use crate::utils::buffer::SharedBufferManager;
use crate::utils::js_interop::post_worker_message;
use crate::NostrError;
use flatbuffers::FlatBufferBuilder;
use futures::channel::mpsc;
use futures::lock::Mutex;
use js_sys::SharedArrayBuffer;
use web_sys::MessagePort;
use rustc_hash::FxHashMap;
use shared::generated::nostr::fb::{self};
use shared::types::network::Request;
use shared::types::nostr::Template;
use shared::Port;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::{info, info_span, warn, Span};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

type Result<T> = std::result::Result<T, NostrError>;

// Tunables
const MAX_INFLIGHT: usize = 24; // Increased from 6 to allow more parallel processing
const STARTUP_DELAY_MS: u32 = 100; // Reduced from 500ms for faster startup
const INITIAL_BACKOFF_MS: u32 = 1; // Reduced from 8ms for tighter polling
const MAX_BACKOFF_MS: u32 = 128; // Reduced from 512ms for more responsive backoff
const BATCH_SIZE: usize = 8; // Process multiple messages in one iteration
const NUM_SHARDS: usize = 10; // Number of shard workers
const SHARD_CAP: usize = BATCH_SIZE * 4; // bounded for backpressure
const SLOW_SHARDS: usize = 2; // last N shards reserved for slow subs

struct Sub {
    pipeline: Arc<Mutex<Pipeline>>,
    buffer: SharedArrayBuffer,
    eosed: bool,
    publish_id: Option<String>,
    forced_shard: Option<usize>, // Optional forced shard routing
}

/// Tracks the origin of a message for debugging/metrics
#[derive(Clone, Copy, Debug)]
enum ShardSource {
    Network,
    Cache,
}

pub struct NetworkManager {
    to_cache: Rc<RefCell<Port>>,
    to_main: Option<MessagePort>,
    publish_manager: publish::PublishManager,
    subscription_manager: subscription::SubscriptionManager,
    subscriptions: Arc<RwLock<FxHashMap<String, Sub>>>,
    crypto_client: Arc<CryptoClient>,
    slow_rr: Rc<RefCell<usize>>, // round-robin index for reserved slow shards
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
        parser: Arc<Parser>,
        to_cache: Rc<RefCell<Port>>,
        from_connections: mpsc::Receiver<Vec<u8>>,
        from_cache: mpsc::Receiver<Vec<u8>>,
        crypto_client: Arc<CryptoClient>,
        to_main: MessagePort,
    ) -> Self {
        let publish_manager = publish::PublishManager::new(parser.clone());
        let subscription_manager =
            subscription::SubscriptionManager::new(parser.clone(), crypto_client.clone());

        let manager = Self {
            to_cache,
            to_main: Some(to_main),
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
            crypto_client,
            slow_rr: Rc::new(RefCell::new(0)),
        };

        manager.start_response_reader(from_connections, from_cache);
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

    async fn handle_message_batch(
        subs: Arc<RwLock<FxHashMap<String, Sub>>>,
        messages: Vec<(Arc<Vec<u8>>, Span)>,
    ) {
        // Process multiple messages with a single lock acquisition where possible
        for (fb_bytes_arc, span) in messages {
            let _guard = span.enter();
            Self::handle_message_single(subs.clone(), fb_bytes_arc).await;
        }
    }

    async fn handle_message_single(
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
            let guard = match subs.read() {
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
                        info!("EOSE received for sub {} on relay {:?}", sid, url);
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
            fb::MessageType::Eoce => {
                info!("EOCE received for sub {}", sid);
                SharedBufferManager::send_eoce(&buffer).await;
                post_worker_message(&JsValue::from_str(sid));
            }
            fb::MessageType::Raw => {
                // info!("Processing raw message for sub {}", sid);
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
                        // info!("Writing output to buffer for sub {}", sid);
                        let _write_span = info_span!("buffer_write", sub_id = %sid).entered();
                        if let Err(e) = SharedBufferManager::write_to_buffer(&buffer, &output).await
                        {
                            warn!("Buffer write failed for sub {}: {:?}", sid, e);
                        }
                        // Retrieve sub by sid and notify only if it's already EOSEd
                        let should_notify = match subs.read() {
                            Ok(g) => g.get(sid).map(|s| s.eosed).unwrap_or(false),
                            Err(_) => false,
                        };

                        info!("Notifying after EOSE for sub {}", sid);

                        if should_notify {
                            post_worker_message(&JsValue::from_str(sid));
                        }
                    }
                    Ok(None) => {
                        // info!("Event dropped by pipeline for sub {}", sid); /* dropped by pipeline */
                    }
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }

                // Once Pipeline accepts bytes, replace with:
                // match pipeline_guard.process_worker_message(&fb_bytes_arc).await { ... }
            }
            fb::MessageType::ParsedNostrEvent => {
                let mut pipeline_guard = pipeline_arc.lock().await;

                match pipeline_guard
                    .process_cached_batch(&[fb_bytes_arc.as_ref().clone()])
                    .await
                {
                    Ok(outputs) => {
                        // Process each output in the vector
                        for output in outputs {
                            let _write_span =
                                info_span!("cache_buffer_write", sub_id = %sid).entered();
                            if let Err(e) =
                                SharedBufferManager::write_to_buffer(&buffer, &output).await
                            {
                                warn!("Buffer write failed for sub {}: {:?}", sid, e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }
            }
            fb::MessageType::NostrEvent => {
                let mut pipeline_guard = pipeline_arc.lock().await;
                match pipeline_guard
                    .process_bytes(fb_bytes_arc.as_ref().as_slice())
                    .await
                {
                    Ok(Some(output)) => {
                        // info!("Writing output to buffer for sub {}", sid);
                        let _write_span = info_span!("buffer_write", sub_id = %sid).entered();
                        if let Err(e) = SharedBufferManager::write_to_buffer(&buffer, &output).await
                        {
                            warn!("Buffer write failed for sub {}: {:?}", sid, e);
                        }
                    }
                    Ok(None) => {
                        // info!("Event dropped by pipeline for sub {}", sid); /* dropped by pipeline */
                    }
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }
            }
            _ => {
                // Ignore other message types in this reader
            }
        }
    }

    fn start_response_reader(
        &self,
        from_connections: mpsc::Receiver<Vec<u8>>,
        from_cache: mpsc::Receiver<Vec<u8>>,
    ) {
        use futures::{channel::mpsc, FutureExt, SinkExt, StreamExt};
        use std::hash::{Hash, Hasher};
        use tracing::{info_span, warn};

        let subs = self.subscriptions.clone();

        // Sharded executors: fixed number of long-lived workers
        // Using module-level NUM_SHARDS and SHARD_CAP

        // Create shard channels + workers
        let mut shard_senders = Vec::with_capacity(NUM_SHARDS);
        for shard_idx in 0..NUM_SHARDS {
            let (tx, mut rx) = mpsc::channel::<(std::sync::Arc<Vec<u8>>, ShardSource, tracing::Span)>(SHARD_CAP);
            let subs_clone = subs.clone();

            spawn_local(async move {
                let mut local_batch: Vec<(std::sync::Arc<Vec<u8>>, ShardSource, tracing::Span)> =
                    Vec::with_capacity(BATCH_SIZE);

                loop {
                    local_batch.clear();

                    // Drive progress by awaiting at least one message
                    match rx.next().await {
                        Some(item) => local_batch.push(item),
                        None => {
                            warn!("Shard {} exited (channel closed)", shard_idx);
                            break;
                        }
                    }

                    // Opportunistically drain more without awaiting
                    while local_batch.len() < BATCH_SIZE {
                        match rx.next().now_or_never() {
                            Some(Some(item)) => local_batch.push(item),
                            Some(None) => break, // channel closed
                            None => break,       // nothing ready
                        }
                    }

                    // Process in-order within the shard to preserve per-sub ordering
                    let batch = std::mem::take(&mut local_batch);
                    // Extract just the bytes and span for processing
                    let processed_batch: Vec<(std::sync::Arc<Vec<u8>>, tracing::Span)> = batch
                        .into_iter()
                        .map(|(bytes, _source, span)| (bytes, span))
                        .collect();
                    NetworkManager::handle_message_batch(subs_clone.clone(), processed_batch).await;
                }
            });

            shard_senders.push(tx);
        }

        // Distributor task: use select! to race between from_connections and from_cache
        spawn_local(async move {
            use futures::select;

            let mut from_connections = from_connections.fuse();
            let mut from_cache = from_cache.fuse();

            loop {
                // Use select! to race between the two receivers
                let bytes_result = select! {
                    bytes = from_connections.next() => {
                        bytes.map(|b| (b, ShardSource::Network))
                    }
                    bytes = from_cache.next() => {
                        bytes.map(|b| (b, ShardSource::Cache))
                    }
                };

                match bytes_result {
                    Some((bytes, source)) => {
                        // Decode minimally to route by sub_id
                        let wm = match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                            Ok(w) => w,
                            Err(_) => {
                                warn!("WorkerMessage decode failed in distributor; dropping frame");
                                continue;
                            }
                        };
                        let sid = wm.sub_id().unwrap_or("");
                        if sid.is_empty() {
                            warn!("Invalid message: Missing sub_id");
                            continue;
                        }

                        // Sub span for downstream processing
                        let sub_span = info_span!("sub_request", sub_id = %sid);

                        // Compute shard index (respect forced shard if set)
                        let shard_idx = {
                            let forced = subs
                                .read()
                                .ok()
                                .and_then(|g| g.get(sid).and_then(|s| s.forced_shard));
                            if let Some(idx) = forced {
                                idx.min(NUM_SHARDS - 1)
                            } else {
                                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                sid.hash(&mut hasher);
                                // Route non-forced subs only to fast shards (exclude reserved slow shards at the end)
                                let fast_count = NUM_SHARDS.saturating_sub(SLOW_SHARDS);
                                if fast_count > 0 {
                                    (hasher.finish() as usize) % fast_count
                                } else {
                                    // Fallback: if all shards are reserved as slow, spread across all
                                    (hasher.finish() as usize) % NUM_SHARDS
                                }
                            }
                        };

                        // Send to shard with try_send first to avoid stalling distributor; fall back to await to preserve ordering
                        let fb_arc = std::sync::Arc::new(bytes);
                        let mut tx = shard_senders[shard_idx].clone();
                        if let Err(_e) = tx.try_send((fb_arc.clone(), source, sub_span.clone())) {
                            if let Err(e) = tx.send((fb_arc, source, sub_span)).await {
                                warn!("Shard {} send failed: {:?}", shard_idx, e);
                            }
                        }
                    }
                    None => {
                        // Both channels closed
                        warn!("Distributor exiting (both channels closed)");
                        break;
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
        info!("open_subscription: {}", subscription_id);
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
            .process_subscription(
                &subscription_id,
                self.to_cache.clone(),
                parsed_requests,
                config,
            )
            .await?;

        if let Ok(mut w) = self.subscriptions.write() {
            // Determine forced shard based on config.is_slow
            let forced_shard = if config.is_slow() {
                let slow_count = if SLOW_SHARDS == 0 { 1 } else { SLOW_SHARDS };
                let slow_start = NUM_SHARDS.saturating_sub(slow_count);
                let idx = {
                    let mut rr = self.slow_rr.borrow_mut();
                    let v = slow_start + (*rr % slow_count);
                    *rr = rr.wrapping_add(1);
                    v
                };
                Some(idx)
            } else {
                None
            };

            w.insert(
                subscription_id.clone(),
                Sub {
                    pipeline: Arc::new(Mutex::new(pipeline)),
                    buffer: shared_buffer.clone(),
                    eosed: false,
                    // relay_urls: relay_filters.keys().cloned().collect(),
                    publish_id: None,
                    forced_shard,
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
                    event: None,
                    relays: None,
                },
            );

            builder.finish(cache_req, None);
            let bytes = builder.finished_data().to_vec();

            // Send CacheRequest bytes through the MessageChannel port
            let _ = self.to_cache.borrow().send(&bytes);
        }

        Ok(())
    }

    // deprecated, should be called on connections directly
    pub async fn close_subscription(&self, subscription_id: String) -> Result<()> {
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
        let event = self
            .publish_manager
            .publish_event(publish_id.clone(), template)
            .await?;

        if let Ok(mut w) = self.subscriptions.write() {
            w.insert(
                event.id.to_string(),
                Sub {
                    pipeline: Arc::new(Mutex::new(Pipeline::new(vec![], "".to_string()).unwrap())),
                    buffer: shared_buffer.clone(),
                    eosed: false,
                    // relay_urls: all_relays.clone(),
                    publish_id: Some(publish_id.clone()),
                    forced_shard: None,
                },
            );
        } else {
            warn!(
                "Subscriptions lock poisoned while publishing {}",
                publish_id
            );
        }

        {
            let mut builder = FlatBufferBuilder::new();

            let sid = builder.create_string(&event.id.to_string());

            let fb_event = event.build_flatbuffer(&mut builder);

            let relay_offsets: Vec<_> = default_relays
                .iter()
                .map(|r| builder.create_string(r))
                .collect();
            let relay_vec = if relay_offsets.is_empty() {
                None
            } else {
                Some(builder.create_vector(&relay_offsets))
            };

            let cache_req = fb::CacheRequest::create(
                &mut builder,
                &fb::CacheRequestArgs {
                    sub_id: Some(sid),
                    requests: None,
                    event: Some(fb_event),
                    relays: relay_vec,
                },
            );

            builder.finish(cache_req, None);
            let bytes = builder.finished_data().to_vec();

            // Send CacheRequest bytes through the MessageChannel port
            let _ = self.to_cache.borrow().send(&bytes);
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
