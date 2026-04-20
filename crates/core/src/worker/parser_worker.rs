use crate::channel::WorkerChannel;
use crate::generated::nostr::fb;
use crate::network::{publish::PublishManager, subscription::SubscriptionManager};
use crate::parser::Parser;
use crate::pipeline::Pipeline;
use crate::port::Port;
use crate::spawn::spawn_worker;
use crate::types::{network::Request, nostr::Template};
use crate::nostr_error::{NostrError, NostrResult};
use flatbuffers::FlatBufferBuilder;
use futures::channel::mpsc;
use futures::lock::Mutex;
use futures::{FutureExt, SinkExt, StreamExt};
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use tracing::{info, info_span, warn, Span};

#[cfg(target_arch = "wasm32")]
use crate::worker::batch_buffer;
#[cfg(target_arch = "wasm32")]
use web_sys::MessagePort;
#[cfg(not(target_arch = "wasm32"))]
use tokio::sync::mpsc::UnboundedSender;

// Tunables
const MAX_INFLIGHT: usize = 24;
const STARTUP_DELAY_MS: u32 = 100;
const INITIAL_BACKOFF_MS: u32 = 1;
const MAX_BACKOFF_MS: u32 = 128;
const BATCH_SIZE: usize = 8;
const NUM_SHARDS: usize = 10;
const SHARD_CAP: usize = BATCH_SIZE * 4;
const SLOW_SHARDS: usize = 2;

struct Sub {
    pipeline: Arc<Mutex<Pipeline>>,
    eosed: bool,
    publish_id: Option<String>,
    forced_shard: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
enum ShardSource {
    Network,
    Cache,
}

type ShardTask = (String, Arc<Vec<u8>>, ShardSource, Span);
type DispatchTask = (usize, ShardTask);

pub struct ParserWorker {
    parser: Arc<Parser>,
    to_cache: Arc<dyn Port>,
    #[cfg(not(target_arch = "wasm32"))]
    to_main: UnboundedSender<(String, Vec<u8>)>,
    #[cfg(target_arch = "wasm32")]
    to_main: MessagePort,
    publish_manager: PublishManager,
    subscription_manager: SubscriptionManager,
    subscriptions: Arc<RwLock<FxHashMap<String, Sub>>>,
    slow_rr: AtomicUsize,
}

impl ParserWorker {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(
        parser: Arc<Parser>,
        to_cache: Arc<dyn Port>,
        to_main: UnboundedSender<(String, Vec<u8>)>,
    ) -> Self {
        let publish_manager = PublishManager::new(parser.clone());
        let subscription_manager = SubscriptionManager::new(parser.clone());
        Self {
            parser,
            to_cache,
            to_main,
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
            slow_rr: AtomicUsize::new(0),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn new(
        parser: Arc<Parser>,
        to_cache: Arc<dyn Port>,
        to_main: MessagePort,
    ) -> Self {
        let publish_manager = PublishManager::new(parser.clone());
        let subscription_manager = SubscriptionManager::new(parser.clone());
        batch_buffer::init_global_batch_manager(to_main.clone());
        Self {
            parser,
            to_cache,
            to_main,
            publish_manager,
            subscription_manager,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
            slow_rr: AtomicUsize::new(0),
        }
    }

    pub fn run(
        self,
        mut from_engine: Box<dyn WorkerChannel>,
        mut from_connections: Box<dyn WorkerChannel>,
        mut from_cache: Box<dyn WorkerChannel>,
    ) {
        let this = Arc::new(self);

        // Command loop
        let this_cmd = this.clone();
        spawn_worker(async move {
            loop {
                match from_engine.recv().await {
                    Ok(bytes) => {
                        let mm = match flatbuffers::root::<fb::MainMessage>(&bytes) {
                            Ok(m) => m,
                            Err(_) => {
                                warn!("MainMessage decode failed; dropping frame");
                                continue;
                            }
                        };
                        match mm.unpack().content {
                            fb::MainContentT::Subscribe(sub) => {
                                let sub_id = sub.subscription_id;
                                let requests = sub.requests;
                                let config = *sub.config;
                                if let Err(e) = this_cmd.open_subscription(sub_id, requests, config).await {
                                    warn!("open_subscription failed: {:?}", e);
                                }
                            }
                            fb::MainContentT::Unsubscribe(unsub) => {
                                if let Err(e) = this_cmd.close_subscription(unsub.subscription_id).await {
                                    warn!("close_subscription failed: {:?}", e);
                                }
                            }
                            fb::MainContentT::Publish(pub_msg) => {
                                let publish_id = pub_msg.publish_id;
                                let template = template_from_t(&pub_msg.template);
                                let relays = pub_msg.relays;
                                let optimistic = pub_msg.optimistic_subids.unwrap_or_default();
                                if let Err(e) = this_cmd.publish_event(publish_id, &template, &relays, optimistic).await {
                                    warn!("publish_event failed: {:?}", e);
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(_) => {
                        warn!("from_engine channel closed");
                        break;
                    }
                }
            }
        });

        // Sharded executors
        let subs = this.subscriptions.clone();
        let mut shard_senders = Vec::with_capacity(NUM_SHARDS);
        for shard_idx in 0..NUM_SHARDS {
            let (tx, mut rx) = mpsc::channel::<ShardTask>(SHARD_CAP);
            let this_shard = this.clone();

            spawn_worker(async move {
                let mut local_batch: Vec<ShardTask> = Vec::with_capacity(BATCH_SIZE);

                loop {
                    local_batch.clear();

                    match rx.next().await {
                        Some((sid, bytes, source, span)) => {
                            local_batch.push((sid, bytes, source, span));
                        }
                        None => {
                            warn!("Shard {} exited (channel closed)", shard_idx);
                            break;
                        }
                    }

                    while local_batch.len() < BATCH_SIZE {
                        match rx.next().now_or_never() {
                            Some(Some(item)) => local_batch.push(item),
                            Some(None) => break,
                            None => break,
                        }
                    }

                    let batch = std::mem::take(&mut local_batch);
                    for (sid, _bytes, _source, _span) in &batch {
                        info!(
                            "[shard {}] Processing message for sub_id={}",
                            shard_idx, sid
                        );
                    }
                    let processed_batch: Vec<(String, Arc<Vec<u8>>, Span)> = batch
                        .into_iter()
                        .map(|(sid, bytes, _source, span)| (sid, bytes, span))
                        .collect();
                    this_shard.handle_message_batch(processed_batch).await;
                }
            });

            shard_senders.push(tx);
        }

        // Fast/slow lane dispatchers
        let dispatch_cap = NUM_SHARDS * SHARD_CAP;
        let (fast_tx, mut fast_rx) = mpsc::channel::<DispatchTask>(dispatch_cap);
        let (slow_tx, mut slow_rx) = mpsc::channel::<DispatchTask>(dispatch_cap);

        let fast_shard_senders = shard_senders.clone();
        spawn_worker(async move {
            while let Some((shard_idx, task)) = fast_rx.next().await {
                let mut tx = fast_shard_senders[shard_idx].clone();
                if let Err(_e) = tx.try_send(task.clone()) {
                    if let Err(e) = tx.send(task).await {
                        warn!("Fast lane shard {} send failed: {:?}", shard_idx, e);
                    }
                }
            }
            warn!("Fast lane dispatcher exiting (channel closed)");
        });

        let slow_shard_senders = shard_senders.clone();
        spawn_worker(async move {
            while let Some((shard_idx, task)) = slow_rx.next().await {
                let mut tx = slow_shard_senders[shard_idx].clone();
                if let Err(_e) = tx.try_send(task.clone()) {
                    if let Err(e) = tx.send(task).await {
                        warn!("Slow lane shard {} send failed: {:?}", shard_idx, e);
                    }
                }
            }
            warn!("Slow lane dispatcher exiting (channel closed)");
        });

        // Ingress task
        spawn_worker(async move {
            use futures::select;
            loop {
                let bytes_result = select! {
                    result = from_connections.recv().fuse() => {
                        result.ok().map(|b| (b, ShardSource::Network))
                    }
                    result = from_cache.recv().fuse() => {
                        result.ok().map(|b| (b, ShardSource::Cache))
                    }
                };

                match bytes_result {
                    Some((bytes, source)) => {
                        match source {
                            ShardSource::Network => {
                                let wm = match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
                                    Ok(w) => w,
                                    Err(_) => {
                                        warn!("WorkerMessage decode failed from network; dropping frame");
                                        continue;
                                    }
                                };

                                let sid = wm.sub_id().unwrap_or("").to_string();
                                let msg_type = wm.type_();
                                info!(
                                    "[network] Received from network: type={:?}, sub_id={}",
                                    msg_type, sid
                                );
                                if sid.is_empty() {
                                    warn!("Invalid message: Missing sub_id");
                                    continue;
                                }

                                let (shard_idx, is_slow_lane) = {
                                    let forced = subs
                                        .read()
                                        .ok()
                                        .and_then(|g| g.get(&sid).and_then(|s| s.forced_shard));

                                    if let Some(idx) = forced {
                                        let clamped = idx.min(NUM_SHARDS - 1);
                                        let slow_start = NUM_SHARDS.saturating_sub(SLOW_SHARDS.max(1));
                                        (clamped, clamped >= slow_start)
                                    } else {
                                        use std::hash::{Hash, Hasher};
                                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                        sid.hash(&mut hasher);
                                        let fast_count = NUM_SHARDS.saturating_sub(SLOW_SHARDS);
                                        let shard = if fast_count > 0 {
                                            (hasher.finish() as usize) % fast_count
                                        } else {
                                            (hasher.finish() as usize) % NUM_SHARDS
                                        };
                                        (shard, false)
                                    }
                                };

                                info!(
                                    "[network] Dispatching to shard {} (slow={}): sub_id={}",
                                    shard_idx, is_slow_lane, sid
                                );
                                let sub_span = info_span!("sub_request", sub_id = %sid);
                                let task: ShardTask =
                                    (sid, Arc::new(bytes), ShardSource::Network, sub_span);

                                let mut lane_tx = if is_slow_lane {
                                    slow_tx.clone()
                                } else {
                                    fast_tx.clone()
                                };

                                if let Err(_e) = lane_tx.try_send((shard_idx, task.clone())) {
                                    if let Err(e) = lane_tx.send((shard_idx, task)).await {
                                        warn!(
                                            "{} lane enqueue failed for shard {}: {:?}",
                                            if is_slow_lane { "Slow" } else { "Fast" },
                                            shard_idx,
                                            e
                                        );
                                    }
                                }
                            }
                            ShardSource::Cache => {
                                let resp = match flatbuffers::root::<fb::CacheResponse>(&bytes) {
                                    Ok(r) => r,
                                    Err(_) => {
                                        warn!("CacheResponse decode failed from cache; dropping frame");
                                        continue;
                                    }
                                };

                                let sid: String = resp.sub_id().to_string();
                                if sid.is_empty() {
                                    warn!("Invalid cache response: Missing sub_id");
                                    continue;
                                }

                                let (shard_idx, is_slow_lane) = {
                                    let forced = subs
                                        .read()
                                        .ok()
                                        .and_then(|g| g.get(&sid).and_then(|s| s.forced_shard));

                                    if let Some(idx) = forced {
                                        let clamped = idx.min(NUM_SHARDS - 1);
                                        let slow_start = NUM_SHARDS.saturating_sub(SLOW_SHARDS.max(1));
                                        (clamped, clamped >= slow_start)
                                    } else {
                                        use std::hash::{Hash, Hasher};
                                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                                        sid.hash(&mut hasher);
                                        let fast_count = NUM_SHARDS.saturating_sub(SLOW_SHARDS);
                                        let shard = if fast_count > 0 {
                                            (hasher.finish() as usize) % fast_count
                                        } else {
                                            (hasher.finish() as usize) % NUM_SHARDS
                                        };
                                        (shard, false)
                                    }
                                };

                                let payload = resp.payload().map(|p| p.bytes()).unwrap_or(&[]);
                                let cache_sub_span = info_span!("sub_request", sub_id = %sid);

                                let mut lane_tx = if is_slow_lane {
                                    slow_tx.clone()
                                } else {
                                    fast_tx.clone()
                                };

                                if payload.is_empty() {
                                    let eoce_arc = Arc::new(serialize_eoce());
                                    let task: ShardTask =
                                        (sid, eoce_arc, ShardSource::Cache, cache_sub_span);
                                    if let Err(_e) = lane_tx.try_send((shard_idx, task.clone())) {
                                        if let Err(e) = lane_tx.send((shard_idx, task)).await {
                                            warn!(
                                                "{} lane enqueue failed for EOCE shard {}: {:?}",
                                                if is_slow_lane { "Slow" } else { "Fast" },
                                                shard_idx,
                                                e
                                            );
                                        }
                                    }
                                    continue;
                                }

                                let messages = Self::unpack_batched_messages(payload);
                                for msg_bytes in messages.iter() {
                                    let task: ShardTask = (
                                        sid.clone(),
                                        Arc::new(msg_bytes.clone()),
                                        ShardSource::Cache,
                                        cache_sub_span.clone(),
                                    );
                                    if let Err(_e) = lane_tx.try_send((shard_idx, task.clone())) {
                                        if let Err(e) = lane_tx.send((shard_idx, task)).await {
                                            warn!(
                                                "{} lane enqueue failed for cache msg shard {}: {:?}",
                                                if is_slow_lane { "Slow" } else { "Fast" },
                                                shard_idx,
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                    None => {
                        warn!("Ingress distributor exiting (both channels closed)");
                        break;
                    }
                }
            }
        });
    }

    async fn handle_message_batch(
        &self,
        messages: Vec<(String, Arc<Vec<u8>>, Span)>,
    ) {
        for (sid, fb_bytes_arc, span) in messages {
            let _guard = span.enter();
            self.handle_message_single(sid, fb_bytes_arc).await;
        }
    }

    async fn handle_message_single(&self, sid: String, fb_bytes_arc: Arc<Vec<u8>>) {
        info!(
            "[handle_message_single] Processing message for sub_id={}",
            sid
        );
        let wm = match flatbuffers::root::<fb::WorkerMessage>(&fb_bytes_arc) {
            Ok(w) => w,
            Err(_) => {
                warn!("Re-root failed for WorkerMessage (malformed FB)");
                return;
            }
        };

        match wm.type_() {
            fb::MessageType::ConnectionStatus => {
                // handle connection status directly without needing a subscription
                let Some(cs) = wm.content_as_connection_status() else {
                    warn!("WorkerMessage ConnectionStatus missing content");
                    return;
                };
                let url = wm.url().unwrap_or("");
                let status = cs.status();
                let reason = cs.message().unwrap_or("");
                match status {
                    "NOTICE" => {}
                    "AUTH" => {
                        warn!("Auth needed on relay {:?}", url);
                    }
                    "EOSE" => {
                        info!(
                            "[network] Received EOSE for sub_id={} from relay={}",
                            sid, url
                        );
                        let flushed_outputs = if let Ok(guard) = self.subscriptions.read() {
                            if let Some(sub) = guard.get(&sid) {
                                let pipeline_arc = Arc::clone(&sub.pipeline);
                                drop(guard);
                                let mut pipeline_guard = pipeline_arc.lock().await;
                                let outputs = pipeline_guard.flush();
                                pipeline_guard.on_eose();
                                outputs
                            } else {
                                Vec::new()
                            }
                        } else {
                            warn!("Subscriptions lock poisoned");
                            Vec::new()
                        };

                        for output in flushed_outputs {
                            self.send_output_to_main(&sid, &output);
                        }

                        let status_bytes = serialize_connection_status(url, "EOSE", "");
                        self.send_output_to_main(&sid, &status_bytes);
                        self.flush_main(&sid);
                        if let Ok(mut w) = self.subscriptions.write() {
                            if let Some(sub) = w.get_mut(&sid) {
                                sub.eosed = true;
                            }
                        }
                    }
                    "CLOSED" => {}
                    accepted => {
                        let batch_sub_id = if let Ok(guard) = self.subscriptions.read() {
                            if let Some(sub) = guard.get(&sid) {
                                sub.publish_id.clone().unwrap_or_else(|| sid.clone())
                            } else {
                                sid.clone()
                            }
                        } else {
                            sid.clone()
                        };

                        let status_bytes = serialize_connection_status(url, accepted, reason);
                        self.send_output_to_main(&batch_sub_id, &status_bytes);
                        self.flush_main(&batch_sub_id);
                    }
                }
                return;
            }
            _ => {}
        }

        if sid.is_empty() {
            warn!("Invalid message: Missing sub_id");
            return;
        }

        let (pipeline_arc, publish_id, eosed) = {
            let guard = match self.subscriptions.read() {
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
                // Handled above before subscription lookup
            }
            fb::MessageType::Eoce => {
                let flushed_outputs = {
                    let mut pipeline_guard = pipeline_arc.lock().await;
                    pipeline_guard.flush()
                };

                for output in flushed_outputs {
                    self.send_output_to_main(&sid, &output);
                }

                let eoce_bytes = serialize_eoce();
                self.send_output_to_main(&sid, &eoce_bytes);
                self.flush_main(&sid);
            }
            fb::MessageType::Raw => {
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
                        self.send_output_to_main(&sid, &output);
                        if eosed {
                            self.flush_main(&sid);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }
            }
            fb::MessageType::ParsedNostrEvent => {
                let mut pipeline_guard = pipeline_arc.lock().await;
                match pipeline_guard
                    .process_cached_batch(&[fb_bytes_arc.as_ref().clone()])
                    .await
                {
                    Ok(outputs) => {
                        for output in outputs.iter() {
                            self.send_output_to_main(&sid, output);
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
                        self.send_output_to_main(&sid, &output);
                        if eosed {
                            self.flush_main(&sid);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => {
                        warn!("Pipeline process failed for sub {}: {:?}", sid, e);
                    }
                }
            }
            _ => {}
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        requests: Vec<fb::RequestT>,
        config: fb::SubscriptionConfigT,
    ) -> NostrResult<()> {
        if self
            .subscriptions
            .read()
            .map(|g| g.contains_key(&subscription_id))
            .unwrap_or(false)
        {
            return Ok(());
        }

        let parsed_requests: Vec<Request> = requests.iter().map(request_from_t).collect();

        let mut config_builder = FlatBufferBuilder::new();
        let config_offset = config.pack(&mut config_builder);
        config_builder.finish(config_offset, None);
        let config_fb = flatbuffers::root::<fb::SubscriptionConfig>(config_builder.finished_data()).unwrap();

        let pipeline = self
            .subscription_manager
            .process_subscription(
                &subscription_id,
                self.to_cache.clone(),
                parsed_requests,
                &config_fb,
            )
            .await?;

        if let Some(parent_id) = config.pagination.as_deref() {
            if let Ok(guard) = self.subscriptions.read() {
                if let Some(parent_sub) = guard.get(parent_id) {
                    let parent_pipeline_arc = Arc::clone(&parent_sub.pipeline);
                    drop(guard);
                    let parent_pipeline = parent_pipeline_arc.lock().await;
                    pipeline.clone_state_from(&parent_pipeline);
                    info!(
                        "Cloned pipeline state from parent subscription '{}' to '{}'",
                        parent_id, subscription_id
                    );
                } else {
                    warn!(
                        "Parent subscription '{}' not found for pagination subscription '{}'",
                        parent_id, subscription_id
                    );
                }
            }
        }

        if let Ok(mut w) = self.subscriptions.write() {
            let forced_shard = if config.is_slow {
                let slow_count = if SLOW_SHARDS == 0 { 1 } else { SLOW_SHARDS };
                let slow_start = NUM_SHARDS.saturating_sub(slow_count);
                let rr = self.slow_rr.fetch_add(1, Ordering::Relaxed);
                let v = slow_start + (rr % slow_count);
                Some(v)
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

        #[cfg(target_arch = "wasm32")]
        batch_buffer::create_batch_buffer(&subscription_id);

        {
            let mut builder = FlatBufferBuilder::new();
            let sid = builder.create_string(&subscription_id);
            let req_offsets: Vec<_> = requests
                .iter()
                .map(|r| r.pack(&mut builder))
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
            let _ = self.to_cache.send(&bytes);
        }

        Ok(())
    }

    pub async fn close_subscription(&self, subscription_id: String) -> NostrResult<()> {
        if let Ok(mut w) = self.subscriptions.write() {
            w.remove(&subscription_id);
        }

        #[cfg(target_arch = "wasm32")]
        batch_buffer::remove_batch_buffer(&subscription_id);

        Ok(())
    }

    async fn inject_optimistic_event(
        &self,
        sub_id: &str,
        event_json: &str,
    ) -> NostrResult<()> {
        let pipelines: Vec<(String, Arc<Mutex<Pipeline>>)> = {
            let guard = self.subscriptions.read().map_err(|_| {
                NostrError::Other("Subscriptions lock poisoned".into())
            })?;
            guard
                .iter()
                .filter(|(key, _)| key.contains(sub_id))
                .map(|(key, sub)| (key.clone(), Arc::clone(&sub.pipeline)))
                .collect()
        };

        if pipelines.is_empty() {
            return Err(NostrError::Other(format!(
                "Optimistic sub_id {} not found",
                sub_id
            )));
        }

        let mut any_success = false;
        for (actual_sub_id, pipeline_arc) in pipelines {
            let mut pipeline_guard = pipeline_arc.lock().await;
            match pipeline_guard.process(event_json).await {
                Ok(Some(output)) => {
                    self.send_output_to_main(&actual_sub_id, &output);
                    self.flush_main(&actual_sub_id);
                    any_success = true;
                }
                Ok(None) => {
                    info!(
                        "Optimistic event dropped by pipeline for sub {}",
                        actual_sub_id
                    );
                    any_success = true;
                }
                Err(e) => {
                    warn!(
                        "Optimistic event failed for sub {}: {}",
                        actual_sub_id, e
                    );
                }
            }
        }

        if any_success {
            Ok(())
        } else {
            Err(NostrError::Pipeline(format!(
                "All optimistic updates failed for sub_id {}",
                sub_id
            )))
        }
    }

    pub async fn publish_event(
        &self,
        publish_id: String,
        template: &Template,
        default_relays: &Vec<String>,
        optimistic_subids: Vec<String>,
    ) -> NostrResult<()> {
        info!(
            "publish_event: publish_id={}, default_relays={:?}, optimistic_subids={:?}",
            publish_id, default_relays, optimistic_subids
        );

        let event = self
            .publish_manager
            .publish_event(publish_id.clone(), template)
            .await?;

        if !optimistic_subids.is_empty() {
            let event_json = event.to_json();
            for sub_id in &optimistic_subids {
                if let Err(e) = self.inject_optimistic_event(sub_id, &event_json).await {
                    warn!("Failed optimistic update for {}: {}", sub_id, e);
                } else {
                    info!("Sent optimistic update to sub {}", sub_id);
                }
            }
        }

        let event_id = event.id.to_string();
        info!("publish_event: event signed successfully, id={}", event_id);
        if let Ok(mut w) = self.subscriptions.write() {
            w.insert(
                event_id.clone(),
                Sub {
                    pipeline: Arc::new(Mutex::new(Pipeline::new(vec![], "".to_string()).unwrap())),
                    eosed: false,
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

        #[cfg(target_arch = "wasm32")]
        batch_buffer::create_batch_buffer(&publish_id);

        {
            let mut builder = FlatBufferBuilder::new();
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

            info!(
                "publish_event: sending CacheRequest to cache, event_id={}, bytes={}",
                event_id,
                bytes.len()
            );

            if let Err(e) = self.to_cache.send(&bytes) {
                warn!("publish_event: failed to send to cache: {}", e);
            } else {
                info!("publish_event: CacheRequest sent to cache successfully");
            }
        }

        Ok(())
    }

    fn unpack_batched_messages(payload: &[u8]) -> Vec<Vec<u8>> {
        let mut messages = Vec::new();
        let mut offset = 0;

        while offset + 4 <= payload.len() {
            let len = u32::from_le_bytes([
                payload[offset],
                payload[offset + 1],
                payload[offset + 2],
                payload[offset + 3],
            ]) as usize;

            if len == 0 || offset + 4 + len > payload.len() {
                warn!(
                    "Invalid batched message length: {} at offset {} (payload {} bytes)",
                    len,
                    offset,
                    payload.len()
                );
                break;
            }

            let msg_bytes = payload[offset + 4..offset + 4 + len].to_vec();
            messages.push(msg_bytes);

            offset += 4 + len;
        }

        messages
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn send_output_to_main(&self, sub_id: &str, data: &[u8]) {
        let _ = self.to_main.send((sub_id.to_string(), data.to_vec()));
    }

    #[cfg(target_arch = "wasm32")]
    fn send_output_to_main(&self, sub_id: &str, data: &[u8]) {
        batch_buffer::add_message_to_batch(sub_id, data);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn flush_main(&self, _sub_id: &str) {}

    #[cfg(target_arch = "wasm32")]
    fn flush_main(&self, sub_id: &str) {
        batch_buffer::flush_batch(sub_id);
    }
}

fn request_from_t(rt: &fb::RequestT) -> Request {
    Request {
        ids: rt.ids.clone().unwrap_or_default(),
        authors: rt.authors.clone().unwrap_or_default(),
        kinds: rt.kinds.clone().unwrap_or_default().into_iter().map(|k| k as i32).collect(),
        tags: {
            let mut map = FxHashMap::default();
            if let Some(ref tags) = rt.tags {
                for sv in tags {
                    if let Some(ref items) = sv.items {
                        if items.len() >= 2 {
                            map.insert(items[0].clone(), items[1..].to_vec());
                        }
                    }
                }
            }
            map
        },
        since: if rt.since != 0 { Some(rt.since) } else { None },
        until: if rt.until != 0 { Some(rt.until) } else { None },
        limit: if rt.limit != 0 { Some(rt.limit) } else { None },
        search: rt.search.clone(),
        relays: rt.relays.clone().unwrap_or_default(),
        close_on_eose: rt.close_on_eose,
        cache_first: rt.cache_first,
        no_cache: rt.no_cache,
        max_relays: rt.max_relays as u32,
    }
}

fn template_from_t(tt: &fb::TemplateT) -> Template {
    Template {
        kind: tt.kind,
        content: tt.content.clone(),
        tags: tt.tags.iter().filter_map(|sv| sv.items.clone()).collect(),
        created_at: tt.created_at as u64,
    }
}

fn serialize_connection_status(relay_url: &str, status: &str, message: &str) -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();

    let relay_url_offset = builder.create_string(relay_url);
    let status_offset = builder.create_string(status);
    let message_offset = builder.create_string(message);

    let conn_status_args = fb::ConnectionStatusArgs {
        relay_url: Some(relay_url_offset),
        status: Some(status_offset),
        message: Some(message_offset),
    };
    let conn_status_offset = fb::ConnectionStatus::create(&mut builder, &conn_status_args);

    let message_args = fb::WorkerMessageArgs {
        sub_id: None,
        url: None,
        type_: fb::MessageType::ConnectionStatus,
        content_type: fb::Message::ConnectionStatus,
        content: Some(conn_status_offset.as_union_value()),
    };
    let root = fb::WorkerMessage::create(&mut builder, &message_args);
    builder.finish(root, None);

    builder.finished_data().to_vec()
}

fn serialize_eoce() -> Vec<u8> {
    let mut builder = FlatBufferBuilder::new();

    let subscription_id = builder.create_string("");
    let eoce_args = fb::EoceArgs {
        subscription_id: Some(subscription_id),
    };
    let eoce_offset = fb::Eoce::create(&mut builder, &eoce_args);

    let message_args = fb::WorkerMessageArgs {
        sub_id: None,
        url: None,
        type_: fb::MessageType::Eoce,
        content_type: fb::Message::Eoce,
        content: Some(eoce_offset.as_union_value()),
    };
    let root = fb::WorkerMessage::create(&mut builder, &message_args);
    builder.finish(root, None);

    builder.finished_data().to_vec()
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::channel::{ChannelPort, TokioWorkerChannel};
    use crate::parser::Parser;

    // Helper: Build a Subscribe MainMessage
    fn build_subscribe_message(sub_id: &str, requests: Vec<fb::RequestT>) -> Vec<u8> {
        let mut builder = FlatBufferBuilder::new();
        let sid = builder.create_string(sub_id);

        let req_offsets: Vec<_> = requests.iter().map(|r| r.pack(&mut builder)).collect();
        let req_vec = builder.create_vector(&req_offsets);

        let config = fb::SubscriptionConfigT::default();
        let config_offset = config.pack(&mut builder);

        let sub_args = fb::SubscribeArgs {
            subscription_id: Some(sid),
            requests: Some(req_vec),
            config: Some(config_offset),
        };
        let sub_offset = fb::Subscribe::create(&mut builder, &sub_args);

        let main_args = fb::MainMessageArgs {
            content_type: fb::MainContent::Subscribe,
            content: Some(sub_offset.as_union_value()),
        };
        let main_msg = fb::MainMessage::create(&mut builder, &main_args);
        builder.finish(main_msg, None);

        builder.finished_data().to_vec()
    }

    // Helper: Build a Raw WorkerMessage
    fn build_raw_worker_message(sub_id: &str, url: &str, raw_text: &str) -> Vec<u8> {
        let mut builder = FlatBufferBuilder::new();
        let sid = builder.create_string(sub_id);
        let url_off = builder.create_string(url);
        let raw_str = builder.create_string(raw_text);

        let raw_args = fb::RawArgs { raw: Some(raw_str) };
        let raw_offset = fb::Raw::create(&mut builder, &raw_args);

        let msg_args = fb::WorkerMessageArgs {
            sub_id: Some(sid),
            url: Some(url_off),
            type_: fb::MessageType::Raw,
            content_type: fb::Message::Raw,
            content: Some(raw_offset.as_union_value()),
        };
        let wm = fb::WorkerMessage::create(&mut builder, &msg_args);
        builder.finish(wm, None);

        builder.finished_data().to_vec()
    }

    // Helper: Build an EOSE WorkerMessage (ConnectionStatus type)
    // The URL must be in the top-level WorkerMessage.url field for the parser to extract it
    fn build_eose_worker_message(sub_id: &str, url: &str) -> Vec<u8> {
        let mut builder = FlatBufferBuilder::new();
        let sid = builder.create_string(sub_id);
        let url_off = builder.create_string(url);
        let status_str = builder.create_string("EOSE");
        let message_str = builder.create_string("");

        let conn_args = fb::ConnectionStatusArgs {
            relay_url: Some(url_off),
            status: Some(status_str),
            message: Some(message_str),
        };
        let conn_offset = fb::ConnectionStatus::create(&mut builder, &conn_args);

        let msg_args = fb::WorkerMessageArgs {
            sub_id: Some(sid),
            url: Some(url_off),
            type_: fb::MessageType::ConnectionStatus,
            content_type: fb::Message::ConnectionStatus,
            content: Some(conn_offset.as_union_value()),
        };
        let wm = fb::WorkerMessage::create(&mut builder, &msg_args);
        builder.finish(wm, None);

        builder.finished_data().to_vec()
    }

    // Mock WorkerChannel implementation for testing
    struct MockWorkerChannel {
        sender: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
        receiver: Arc<Mutex<tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>>>,
    }

    #[async_trait::async_trait]
    impl WorkerChannel for MockWorkerChannel {
        async fn recv(&mut self) -> std::result::Result<Vec<u8>, crate::channel::ChannelError> {
            let mut guard = self.receiver.lock().await;
            guard.recv().await.ok_or(crate::channel::ChannelError::ChannelClosed)
        }

        async fn send(&self, bytes: &[u8]) -> std::result::Result<(), crate::channel::ChannelError> {
            self.sender.send(bytes.to_vec()).map_err(|_| crate::channel::ChannelError::ChannelClosed)
        }

        fn clone_sender(&self) -> Box<dyn crate::channel::WorkerChannelSender> {
            Box::new(self.sender.clone())
        }
    }

    // CapturePort is a Port implementation that captures sent messages
    struct CapturePort {
        inner: Arc<dyn Port>,
        capture: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    }

    impl Port for CapturePort {
        fn send(&self, bytes: &[u8]) -> Result<(), String> {
            let _ = self.capture.send(bytes.to_vec());
            self.inner.send(bytes)
        }
    }

    #[tokio::test]
    async fn test_eose_from_multiple_relays() {
        let local = tokio::task::LocalSet::new();
        local.run_until(async {
            let (to_main_tx, mut to_main_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();
            let (to_cache_a, _to_cache_b) = TokioWorkerChannel::new_pair();
            
            let (cache_capture_tx, mut cache_capture_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            
            let capture_port: Arc<dyn Port> = Arc::new(CapturePort {
                inner: Arc::new(ChannelPort::new(to_cache_a.clone_sender())),
                capture: cache_capture_tx,
            });

            let parser = Arc::new(Parser::new(None));
            let worker = ParserWorker::new(parser, capture_port, to_main_tx);

            let (from_engine_tx, from_engine_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_engine = Box::new(MockWorkerChannel {
                sender: from_engine_tx.clone(),
                receiver: Arc::new(Mutex::new(from_engine_rx)),
            });

            let (conn_tx, conn_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_connections = Box::new(MockWorkerChannel {
                sender: conn_tx.clone(),
                receiver: Arc::new(Mutex::new(conn_rx)),
            });

            let (cache_tx, cache_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_cache = Box::new(MockWorkerChannel {
                sender: cache_tx,
                receiver: Arc::new(Mutex::new(cache_rx)),
            });

            tokio::task::spawn_local(async move {
                worker.run(from_engine, from_connections, from_cache);
            });

            // Create subscription
            let sub_msg = build_subscribe_message("multi_eose_sub", vec![]);
            from_engine_tx.send(sub_msg).unwrap();
            let _ = cache_capture_rx.recv().await.expect("cache capture channel closed");

            // Send EOSE from 3 different relays
            let relays = ["wss://r1", "wss://r2", "wss://r3"];
            for relay in &relays {
                let eose_msg = build_eose_worker_message("multi_eose_sub", relay);
                conn_tx.send(eose_msg).unwrap();
            }

            // Collect all 3 EOSE status messages with timeout
            let mut received_relays = vec![];
            for _ in 0..3 {
                let result = tokio::time::timeout(
                    tokio::time::Duration::from_secs(2),
                    to_main_rx.recv()
                ).await;
                let (sub_id, data) = result.expect("Timeout waiting for EOSE message").expect("to_main channel closed");
                assert_eq!(sub_id, "multi_eose_sub");
                let wm = flatbuffers::root::<fb::WorkerMessage>(&data).unwrap();
                assert_eq!(wm.type_(), fb::MessageType::ConnectionStatus);
                let cs = wm.content_as_connection_status().unwrap();
                assert_eq!(cs.status(), "EOSE");
                received_relays.push(cs.relay_url().to_string());
            }

            // Verify all 3 relay URLs were received
            assert_eq!(received_relays.len(), 3);
            for relay in &relays {
                assert!(received_relays.contains(&relay.to_string()), "Should have received EOSE from {}", relay);
            }
        }).await;
    }

    #[tokio::test]
    async fn test_partial_eose_some_relays_slow() {
        let local = tokio::task::LocalSet::new();
        local.run_until(async {
            let (to_main_tx, mut to_main_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();
            let (to_cache_a, _to_cache_b) = TokioWorkerChannel::new_pair();
            
            let (cache_capture_tx, mut cache_capture_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            
            let capture_port: Arc<dyn Port> = Arc::new(CapturePort {
                inner: Arc::new(ChannelPort::new(to_cache_a.clone_sender())),
                capture: cache_capture_tx,
            });

            let parser = Arc::new(Parser::new(None));
            let worker = ParserWorker::new(parser, capture_port, to_main_tx);

            let subscriptions = worker.subscriptions.clone();

            let (from_engine_tx, from_engine_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_engine = Box::new(MockWorkerChannel {
                sender: from_engine_tx.clone(),
                receiver: Arc::new(Mutex::new(from_engine_rx)),
            });

            let (conn_tx, conn_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_connections = Box::new(MockWorkerChannel {
                sender: conn_tx.clone(),
                receiver: Arc::new(Mutex::new(conn_rx)),
            });

            let (cache_tx, cache_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_cache = Box::new(MockWorkerChannel {
                sender: cache_tx,
                receiver: Arc::new(Mutex::new(cache_rx)),
            });

            tokio::task::spawn_local(async move {
                worker.run(from_engine, from_connections, from_cache);
            });

            // Create subscription
            let sub_msg = build_subscribe_message("partial_eose_sub", vec![]);
            from_engine_tx.send(sub_msg).unwrap();
            let _ = cache_capture_rx.recv().await.expect("cache capture channel closed");

            // Send a Raw event from relay r1
            let raw_msg = build_raw_worker_message(
                "partial_eose_sub",
                "wss://r1",
                r#"["EVENT","partial_eose_sub",{"id":"event1","pubkey":"npub1","created_at":1234567890,"kind":1,"content":"test"}]"#
            );
            conn_tx.send(raw_msg).unwrap();

            // Wait for event processing
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            // Send EOSE only from relay r2 (not r1)
            let eose_msg = build_eose_worker_message("partial_eose_sub", "wss://r2");
            conn_tx.send(eose_msg).unwrap();

            // Collect EOSE status
            let (sub_id, data) = to_main_rx.recv().await.expect("to_main channel closed");
            assert_eq!(sub_id, "partial_eose_sub");
            let wm = flatbuffers::root::<fb::WorkerMessage>(&data).unwrap();
            assert_eq!(wm.type_(), fb::MessageType::ConnectionStatus);
            let cs = wm.content_as_connection_status().unwrap();
            assert_eq!(cs.status(), "EOSE");
            assert_eq!(cs.relay_url(), "wss://r2");

            // Verify subscription.eosed becomes true after EOSE
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let subs = subscriptions.read().unwrap();
            let sub = subs.get("partial_eose_sub").expect("subscription should exist");
            assert!(sub.eosed, "subscription should be marked as eosed after EOSE");
        }).await;
    }

    #[tokio::test]
    async fn test_eose_after_events_flushes_buffered() {
        let local = tokio::task::LocalSet::new();
        local.run_until(async {
            let (to_main_tx, mut to_main_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();
            let (to_cache_a, _to_cache_b) = TokioWorkerChannel::new_pair();
            
            let (cache_capture_tx, mut cache_capture_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            
            let capture_port: Arc<dyn Port> = Arc::new(CapturePort {
                inner: Arc::new(ChannelPort::new(to_cache_a.clone_sender())),
                capture: cache_capture_tx,
            });

            let parser = Arc::new(Parser::new(None));
            let worker = ParserWorker::new(parser, capture_port, to_main_tx);

            let (from_engine_tx, from_engine_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_engine = Box::new(MockWorkerChannel {
                sender: from_engine_tx.clone(),
                receiver: Arc::new(Mutex::new(from_engine_rx)),
            });

            let (conn_tx, conn_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_connections = Box::new(MockWorkerChannel {
                sender: conn_tx.clone(),
                receiver: Arc::new(Mutex::new(conn_rx)),
            });

            let (cache_tx, cache_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_cache = Box::new(MockWorkerChannel {
                sender: cache_tx,
                receiver: Arc::new(Mutex::new(cache_rx)),
            });

            tokio::task::spawn_local(async move {
                worker.run(from_engine, from_connections, from_cache);
            });

            // Create subscription
            let sub_msg = build_subscribe_message("flush_eose_sub", vec![]);
            from_engine_tx.send(sub_msg).unwrap();
            let _ = cache_capture_rx.recv().await.expect("cache capture channel closed");

            // Send multiple Raw event messages
            for i in 0..3 {
                let raw_msg = build_raw_worker_message(
                    "flush_eose_sub",
                    "wss://relay.example.com",
                    &format!(r#"["EVENT","flush_eose_sub",{{"id":"event{}","pubkey":"npub{}","created_at":1234567890,"kind":1,"content":"test{}"}}]"#, i, i, i)
                );
                conn_tx.send(raw_msg).unwrap();
            }

            // Wait for event processing
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

            // Send EOSE
            let eose_msg = build_eose_worker_message("flush_eose_sub", "wss://relay.example.com");
            conn_tx.send(eose_msg).unwrap();

            // Receive EOSE status message (pipeline flush output + EOSE status)
            let (sub_id, data) = to_main_rx.recv().await.expect("to_main channel closed");
            assert_eq!(sub_id, "flush_eose_sub");
            
            // Verify it's a valid WorkerMessage (could be flushed output or EOSE status)
            let wm = flatbuffers::root::<fb::WorkerMessage>(&data).unwrap();
            // After EOSE, we should get ConnectionStatus type with EOSE
            assert_eq!(wm.type_(), fb::MessageType::ConnectionStatus);
            let cs = wm.content_as_connection_status().unwrap();
            assert_eq!(cs.status(), "EOSE");
        }).await;
    }

    #[tokio::test]
    async fn test_eose_with_empty_pipeline() {
        let local = tokio::task::LocalSet::new();
        local.run_until(async {
            let (to_main_tx, mut to_main_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();
            let (to_cache_a, _to_cache_b) = TokioWorkerChannel::new_pair();
            
            let (cache_capture_tx, mut cache_capture_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            
            let capture_port: Arc<dyn Port> = Arc::new(CapturePort {
                inner: Arc::new(ChannelPort::new(to_cache_a.clone_sender())),
                capture: cache_capture_tx,
            });

            let parser = Arc::new(Parser::new(None));
            let worker = ParserWorker::new(parser, capture_port, to_main_tx);

            let subscriptions = worker.subscriptions.clone();

            let (from_engine_tx, from_engine_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_engine = Box::new(MockWorkerChannel {
                sender: from_engine_tx.clone(),
                receiver: Arc::new(Mutex::new(from_engine_rx)),
            });

            let (conn_tx, conn_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_connections = Box::new(MockWorkerChannel {
                sender: conn_tx.clone(),
                receiver: Arc::new(Mutex::new(conn_rx)),
            });

            let (cache_tx, cache_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_cache = Box::new(MockWorkerChannel {
                sender: cache_tx,
                receiver: Arc::new(Mutex::new(cache_rx)),
            });

            tokio::task::spawn_local(async move {
                worker.run(from_engine, from_connections, from_cache);
            });

            // Create subscription
            let sub_msg = build_subscribe_message("empty_eose_sub", vec![]);
            from_engine_tx.send(sub_msg).unwrap();
            let _ = cache_capture_rx.recv().await.expect("cache capture channel closed");

            // Send EOSE immediately with no prior events
            let eose_msg = build_eose_worker_message("empty_eose_sub", "wss://relay.example.com");
            conn_tx.send(eose_msg).unwrap();

            // Should receive EOSE status without crashing
            let (sub_id, data) = to_main_rx.recv().await.expect("to_main channel closed");
            assert_eq!(sub_id, "empty_eose_sub");
            let wm = flatbuffers::root::<fb::WorkerMessage>(&data).unwrap();
            assert_eq!(wm.type_(), fb::MessageType::ConnectionStatus);
            let cs = wm.content_as_connection_status().unwrap();
            assert_eq!(cs.status(), "EOSE");

            // Verify subscription.eosed is set
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            let subs = subscriptions.read().unwrap();
            let sub = subs.get("empty_eose_sub").expect("subscription should exist");
            assert!(sub.eosed, "subscription should be marked as eosed after EOSE on empty pipeline");
        }).await;
    }

    #[tokio::test]
    async fn test_pipeline_flush_idempotent() {
        let local = tokio::task::LocalSet::new();
        local.run_until(async {
            let (to_main_tx, mut to_main_rx) = tokio::sync::mpsc::unbounded_channel::<(String, Vec<u8>)>();
            let (to_cache_a, _to_cache_b) = TokioWorkerChannel::new_pair();
            
            let (cache_capture_tx, mut cache_capture_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            
            let capture_port: Arc<dyn Port> = Arc::new(CapturePort {
                inner: Arc::new(ChannelPort::new(to_cache_a.clone_sender())),
                capture: cache_capture_tx,
            });

            let parser = Arc::new(Parser::new(None));
            let worker = ParserWorker::new(parser, capture_port, to_main_tx);

            let subscriptions = worker.subscriptions.clone();

            let (from_engine_tx, from_engine_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_engine = Box::new(MockWorkerChannel {
                sender: from_engine_tx.clone(),
                receiver: Arc::new(Mutex::new(from_engine_rx)),
            });

            let (conn_tx, conn_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_connections = Box::new(MockWorkerChannel {
                sender: conn_tx.clone(),
                receiver: Arc::new(Mutex::new(conn_rx)),
            });

            let (cache_tx, cache_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
            let from_cache = Box::new(MockWorkerChannel {
                sender: cache_tx,
                receiver: Arc::new(Mutex::new(cache_rx)),
            });

            tokio::task::spawn_local(async move {
                worker.run(from_engine, from_connections, from_cache);
            });

            // Create subscription
            let sub_msg = build_subscribe_message("idempotent_eose_sub", vec![]);
            from_engine_tx.send(sub_msg).unwrap();
            let _ = cache_capture_rx.recv().await.expect("cache capture channel closed");

            // Send some events
            for i in 0..2 {
                let raw_msg = build_raw_worker_message(
                    "idempotent_eose_sub",
                    "wss://relay.example.com",
                    &format!(r#"["EVENT","idempotent_eose_sub",{{"id":"event{}","pubkey":"npub{}","created_at":1234567890,"kind":1,"content":"test{}"}}]"#, i, i, i)
                );
                conn_tx.send(raw_msg).unwrap();
            }

            // Wait for event processing
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            // Send first EOSE (triggers first flush)
            let eose_msg1 = build_eose_worker_message("idempotent_eose_sub", "wss://relay1.example.com");
            conn_tx.send(eose_msg1).unwrap();

            // Collect first EOSE status
            let (sub_id, data) = to_main_rx.recv().await.expect("to_main channel closed for first EOSE");
            assert_eq!(sub_id, "idempotent_eose_sub");
            let wm = flatbuffers::root::<fb::WorkerMessage>(&data).unwrap();
            assert_eq!(wm.type_(), fb::MessageType::ConnectionStatus);
            let cs = wm.content_as_connection_status().unwrap();
            assert_eq!(cs.status(), "EOSE");
            assert_eq!(cs.relay_url(), "wss://relay1.example.com");

            // Wait for subscription to be marked eosed
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

            // Manually trigger flush again on the pipeline to test idempotency
            {
                let subs = subscriptions.read().unwrap();
                let sub = subs.get("idempotent_eose_sub").expect("subscription should exist");
                let pipeline_arc = Arc::clone(&sub.pipeline);
                drop(subs);
                let mut pipeline_guard = pipeline_arc.lock().await;
                let flushed_outputs = pipeline_guard.flush();
                // Second flush should return empty vec (no double-send of events)
                assert!(flushed_outputs.is_empty(), "Second flush should return empty (idempotent)");
            }

            // Send second EOSE from a different relay - should still work without double-sending
            let eose_msg2 = build_eose_worker_message("idempotent_eose_sub", "wss://relay2.example.com");
            conn_tx.send(eose_msg2).unwrap();

            // Collect second EOSE status
            let (sub_id, data) = to_main_rx.recv().await.expect("to_main channel closed for second EOSE");
            assert_eq!(sub_id, "idempotent_eose_sub");
            let wm = flatbuffers::root::<fb::WorkerMessage>(&data).unwrap();
            assert_eq!(wm.type_(), fb::MessageType::ConnectionStatus);
            let cs = wm.content_as_connection_status().unwrap();
            assert_eq!(cs.status(), "EOSE");
            assert_eq!(cs.relay_url(), "wss://relay2.example.com");
        }).await;
    }
}
