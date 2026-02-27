pub mod interfaces;
pub mod publish;
pub mod subscription;

use crate::crypto_client::CryptoClient;
use crate::parser::Parser;
use crate::pipeline::Pipeline;
use crate::utils::batch_buffer::{
    add_message_to_batch, create_batch_buffer, flush_all_batches, flush_batch,
    init_global_batch_manager, remove_batch_buffer,
};
use crate::utils::buffer::{serialize_connection_status, serialize_eoce};
use crate::utils::js_interop::post_worker_message;
use crate::NostrError;
use flatbuffers::FlatBufferBuilder;
use futures::channel::mpsc;
use futures::lock::Mutex;
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
use web_sys::MessagePort;

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

        // Initialize the global BatchBufferManager with the MessagePort
        init_global_batch_manager(to_main.clone());

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
        messages: Vec<(String, Arc<Vec<u8>>, Span)>,
    ) {
        // Process multiple messages with a single lock acquisition where possible
        // Each message now includes the sub_id (extracted by distributor from CacheResponse or WorkerMessage)
        for (sid, fb_bytes_arc, span) in messages {
            let _guard = span.enter();
            Self::handle_message_single(subs.clone(), sid, fb_bytes_arc).await;
        }
    }

    async fn handle_message_single(
        subs: Arc<RwLock<FxHashMap<String, Sub>>>,
        sid: String,
        fb_bytes_arc: Arc<Vec<u8>>,
    ) {
        let wm = match flatbuffers::root::<fb::WorkerMessage>(&fb_bytes_arc) {
            Ok(w) => w,
            Err(_) => {
                warn!("Re-root failed for WorkerMessage (malformed FB)");
                return;
            }
        };

        if sid.is_empty() {
            warn!("Invalid message: Missing sub_id");
            return;
        }

        // Extract pipeline, publish_id, and eosed status with short-lived lock
        let (pipeline_arc, publish_id, eosed) = {
            let guard = match subs.read() {
                Ok(g) => g,
                Err(_) => {
                    warn!("Subscriptions lock poisoned");
                    return;
                }
            };
            let Some(sub) = guard.get(&sid) else {
                warn!("Sub not found for {}", sid);
                return;
            };
            (Arc::clone(&sub.pipeline), sub.publish_id.clone(), sub.eosed)
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
                        // Notices logged at debug level only
                    }
                    "AUTH" => {
                        warn!("Auth needed on relay {:?}", url);
                    }
                    "CLOSED" => {
                        // Subscription closed - normal operation
                    }
                    "EOSE" => {
                        // Send via batch buffer for MessageChannel delivery
                        let status_bytes = serialize_connection_status(url, "EOSE", "");
                        add_message_to_batch(&sid, &status_bytes);
                        // EOSE is important - flush immediately
                        flush_batch(&sid);
                        if let Ok(mut w) = subs.write() {
                            if let Some(sub) = w.get_mut(&sid) {
                                sub.eosed = true;
                            }
                        }
                    }
                    "OK" => {
                        // OK: forward to UI, and notify any publish waiter
                        let msg = cs.message().unwrap_or("");

                        // For publishes, translate event.id to publish_id so main thread can find it
                        let batch_sub_id = if let Some(ref pid) = publish_id {
                            pid.clone()
                        } else {
                            sid.clone()
                        };

                        // Send via batch buffer for MessageChannel delivery
                        let status_bytes = serialize_connection_status(url, msg, "");
                        add_message_to_batch(&batch_sub_id, &status_bytes);
                        // OK needs low latency - flush immediately
                        flush_batch(&batch_sub_id);
                    }
                    other => {
                        warn!("Unexpected ConnectionStatus '{}' for sub {}", other, sid);
                    }
                }
            }
            fb::MessageType::Eoce => {
                // Send via batch buffer for MessageChannel delivery
                let eoce_bytes = serialize_eoce();
                add_message_to_batch(&sid, &eoce_bytes);
                // EOCE marks end of cache - flush any batched cache events immediately
                flush_batch(&sid);
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
                        // Send via batch buffer for MessageChannel delivery
                        add_message_to_batch(&sid, &output);
                        // If sub is already eosed, flush immediately to avoid losing events
                        if eosed {
                            flush_batch(&sid);
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
                        for output in outputs.iter() {
                            // Send via batch buffer for MessageChannel delivery
                            add_message_to_batch(&sid, output);
                        }
                        // Note: We don't flush here - EOCE will trigger the flush
                        // This allows batching multiple cached events together
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
                        // Send via batch buffer for MessageChannel delivery
                        add_message_to_batch(&sid, &output);
                    }
                    Ok(None) => {
                        // Event dropped by pipeline
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

    /// Unpack batched WorkerMessages from cache payload
    /// Format: [4-byte len (LE)][WorkerMessage bytes]...
    fn unpack_batched_messages(payload: &[u8]) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();
        let mut offset = 0;
        
        while offset + 4 <= payload.len() {
            // Read 4-byte length (little endian)
            let len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;
            
            if len == 0 || offset + 4 + len > payload.len() {
                warn!("Invalid batched message length: {} at offset {} (payload {} bytes)", 
                      len, offset, payload.len());
                break;
            }
            
            // Extract the WorkerMessage bytes
            let msg_bytes = payload[offset + 4..offset + 4 + len].to_vec();
            messages.push(msg_bytes);
            
            offset += 4 + len;
        }
        
        messages
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
        // Channel carries: (sub_id, payload_bytes, source, span)
        let mut shard_senders = Vec::with_capacity(NUM_SHARDS);
        for shard_idx in 0..NUM_SHARDS {
            let (tx, mut rx) =
                mpsc::channel::<(String, std::sync::Arc<Vec<u8>>, ShardSource, tracing::Span)>(SHARD_CAP);
            let subs_clone = subs.clone();

            spawn_local(async move {
                let mut local_batch: Vec<(String, std::sync::Arc<Vec<u8>>, ShardSource, tracing::Span)> =
                    Vec::with_capacity(BATCH_SIZE);

                loop {
                    local_batch.clear();

                    // Drive progress by awaiting at least one message
                    match rx.next().await {
                        Some((sid, bytes, source, span)) => {
                            local_batch.push((sid, bytes, source, span));
                        }
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
                    // Extract sub_id, bytes, and span for processing
                    let processed_batch: Vec<(String, std::sync::Arc<Vec<u8>>, tracing::Span)> = batch
                        .into_iter()
                        .map(|(sid, bytes, _source, span)| (sid, bytes, span))
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
                        // Try to decode and extract sub_id based on source
                        let (sid, payload_arc) = match source {
                            ShardSource::Network => {
                                // Network sends WorkerMessage directly
                                match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                                    Ok(w) => {
                                        let sid = w.sub_id().unwrap_or("").to_string();
                                        (sid, std::sync::Arc::new(bytes))
                                    }
                                    Err(_) => {
                                        warn!("WorkerMessage decode failed from network; dropping frame");
                                        continue;
                                    }
                                }
                            }
                            ShardSource::Cache => {
                                // Cache sends CacheResponse { sub_id, payload }
                                // Payload format: [4-byte len][WorkerMessage][4-byte len][WorkerMessage]...
                                // Empty payload = EOCE (end of cache events)
                                match flatbuffers::root::<fb::CacheResponse>(&bytes) {
                                    Ok(resp) => {
                                        let sid: String = resp.sub_id().to_string();
                                        let payload = resp.payload().map(|p| p.bytes()).unwrap_or(&[]);
                                        
                                        if payload.is_empty() {
                                            // EOCE - end of cache events, signal to flush
                                            // Send EOCE signal through pipeline (special marker)
                                            let eoce_bytes = serialize_eoce();
                                            add_message_to_batch(&sid, &eoce_bytes);
                                            flush_batch(&sid);
                                            continue; // Don't send to shard, already handled
                                        }
                                        
                                        // Compute shard index for this sub_id
                                        let cache_shard_idx = {
                                            let forced = subs
                                                .read()
                                                .ok()
                                                .and_then(|g| g.get(&sid).and_then(|s| s.forced_shard));
                                            if let Some(idx) = forced {
                                                idx.min(NUM_SHARDS - 1)
                                            } else {
                                                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                                sid.hash(&mut hasher);
                                                let fast_count = NUM_SHARDS.saturating_sub(SLOW_SHARDS);
                                                if fast_count > 0 {
                                                    (hasher.finish() as usize) % fast_count
                                                } else {
                                                    (hasher.finish() as usize) % NUM_SHARDS
                                                }
                                            }
                                        };
                                        
                                        // Unpack batched WorkerMessages and send individually
                                        let messages = Self::unpack_batched_messages(payload);
                                        
                                        // Create span for all messages
                                        let cache_sub_span = info_span!("sub_request", sub_id = %sid);
                                        
                                        // Send each WorkerMessage individually to the shard
                                        for msg_bytes in messages.iter() {
                                            let msg_arc = std::sync::Arc::new(msg_bytes.clone());
                                            let mut tx = shard_senders[cache_shard_idx].clone();
                                            if let Err(_e) = tx.try_send((sid.clone(), msg_arc.clone(), ShardSource::Cache, cache_sub_span.clone())) {
                                                if let Err(e) = tx.send((sid.clone(), msg_arc, ShardSource::Cache, cache_sub_span.clone())).await {
                                                    warn!("Shard {} send failed: {:?}", cache_shard_idx, e);
                                                }
                                            }
                                        }
                                        continue; // Already sent all messages, skip default send below
                                    }
                                    Err(_) => {
                                        warn!("CacheResponse decode failed from cache; dropping frame");
                                        continue;
                                    }
                                }
                            }
                        };
                        
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
                                .and_then(|g| g.get(&sid).and_then(|s| s.forced_shard));
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
                        let mut tx = shard_senders[shard_idx].clone();
                        if let Err(_e) = tx.try_send((sid.clone(), payload_arc.clone(), source, sub_span.clone())) {
                            if let Err(e) = tx.send((sid, payload_arc, source, sub_span)).await {
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

        let mut pipeline = self
            .subscription_manager
            .process_subscription(
                &subscription_id,
                self.to_cache.clone(),
                parsed_requests,
                config,
            )
            .await?;

        // If this is a pagination subscription, clone state from the parent
        if let Some(parent_id) = config.pagination() {
            if let Ok(guard) = self.subscriptions.read() {
                if let Some(parent_sub) = guard.get(parent_id) {
                    let parent_pipeline = parent_sub.pipeline.lock().await;
                    pipeline.clone_state_from(&parent_pipeline);
                    tracing::info!(
                        "Cloned pipeline state from parent subscription '{}' to '{}'",
                        parent_id,
                        subscription_id
                    );
                } else {
                    tracing::warn!(
                        "Parent subscription '{}' not found for pagination subscription '{}'",
                        parent_id,
                        subscription_id
                    );
                }
            }
        }

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
                    eosed: false,
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

        // Create a batch buffer for this subscription to send events via MessageChannel
        create_batch_buffer(&subscription_id);

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
                    parsed_event: None,
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

        // Remove and flush the batch buffer for this subscription
        remove_batch_buffer(&subscription_id);

        Ok(())
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
        default_relays: &Vec<String>,
    ) -> Result<()> {
        info!("publish_event: publish_id={}, default_relays={:?}", publish_id, default_relays);
        
        let event = self
            .publish_manager
            .publish_event(publish_id.clone(), template)
            .await?;

        // Store by event.id so OK responses from connections worker can be routed
        let event_id = event.id.to_string();
        info!("publish_event: event signed successfully, id={}", event_id);
        if let Ok(mut w) = self.subscriptions.write() {
            w.insert(
                event_id.clone(),
                Sub {
                    pipeline: Arc::new(Mutex::new(Pipeline::new(vec![], "".to_string()).unwrap())),
                    eosed: false,
                    publish_id: Some(publish_id.clone()), // Store publish_id for translation
                    forced_shard: None,
                },
            );
        } else {
            warn!(
                "Subscriptions lock poisoned while publishing {}",
                publish_id
            );
        }

        // Create batch buffer using publish_id (main thread looks up by publish_id)
        // OK responses come with event.id, but we translate to publish_id before sending to main
        create_batch_buffer(&publish_id);

        {
            let mut builder = FlatBufferBuilder::new();

            // Use event_id as sub_id (connections worker sends OK with event.id)
            let sid = builder.create_string(&event_id);

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
                    parsed_event: None,
                    relays: relay_vec,
                },
            );

            builder.finish(cache_req, None);
            let bytes = builder.finished_data().to_vec();

            info!("publish_event: sending CacheRequest to cache, event_id={}, bytes={}", event_id, bytes.len());
            
            // Send CacheRequest bytes through the MessageChannel port
            if let Err(e) = self.to_cache.borrow().send(&bytes) {
                warn!("publish_event: failed to send to cache: {:?}", e);
            } else {
                info!("publish_event: CacheRequest sent to cache successfully");
            }
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
