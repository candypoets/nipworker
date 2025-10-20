use crate::db::NostrDB;
use crate::generated::nostr::fb;
use crate::network::cache_processor::CacheProcessor;
use crate::network::interfaces::CacheProcessor as CacheProcessorTrait;
use crate::parser::Parser;
use crate::pipeline::pipes::*;
use crate::pipeline::{PipeType, Pipeline};
use crate::relays::utils::{normalize_relay_url, validate_relay_url};
use crate::types::network::Request;
use crate::types::*;
use crate::utils::buffer::SharedBufferManager;
use crate::utils::js_interop::post_worker_message;
use crate::NostrError;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;

type Result<T> = std::result::Result<T, NostrError>;
use std::sync::{Arc, RwLock};
use tracing::{error, info, warn};
use wasm_bindgen::JsValue;

// Added: async backoff sleep for acquiring permits
use gloo_timers::future::TimeoutFuture;
// Added: lightweight semaphore via atomics
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct SubscriptionManager {
    database: Arc<NostrDB>,
    parser: Arc<Parser>,
    subscriptions: Arc<RwLock<FxHashMap<String, SharedArrayBuffer>>>,
    cache_processor: Arc<CacheProcessor>,
    relay_hints: FxHashMap<String, Vec<String>>,

    // Added: simple concurrency limiter
    permits: Arc<AtomicUsize>,
    max_permits: usize,
}

// RAII guard that releases a permit when dropped
struct PermitGuard {
    permits: Arc<AtomicUsize>,
}
impl Drop for PermitGuard {
    fn drop(&mut self) {
        self.permits.fetch_sub(1, Ordering::Release);
    }
}

impl SubscriptionManager {
    pub fn new(database: Arc<NostrDB>, parser: Arc<Parser>) -> Self {
        let cache_processor = Arc::new(CacheProcessor::new(database.clone(), parser.clone()));

        Self {
            database: database.clone(),
            parser,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
            relay_hints: FxHashMap::default(),
            cache_processor,

            // Added: init limiter (12 max concurrent process_subscription calls)
            permits: Arc::new(AtomicUsize::new(0)),
            max_permits: 12,
        }
    }

    // Acquire one concurrency slot with a short async backoff.
    async fn acquire_permit(&self) -> PermitGuard {
        // small exponential backoff to avoid tight loops
        let mut backoff_ms: u32 = 2;
        loop {
            let current = self.permits.load(Ordering::Relaxed);
            if current < self.max_permits {
                if self
                    .permits
                    .compare_exchange(current, current + 1, Ordering::AcqRel, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
                // CAS failed, retry quickly
            } else {
                // At capacity: wait a bit before trying again
                TimeoutFuture::new(backoff_ms).await;
                backoff_ms = (backoff_ms.saturating_mul(2)).min(128);
            }
            // tiny yield between attempts
            TimeoutFuture::new(0).await;
        }
        PermitGuard {
            permits: self.permits.clone(),
        }
    }

    pub async fn close_subscription(&self, subscription_id: &String) -> Result<()> {
        // self.connection_registry
        //     .close_subscription(&subscription_id)
        //     .await?;

        // drop the reference to the sharedBuffer
        self.subscriptions.write().unwrap().remove(subscription_id);

        Ok(())
    }

    pub async fn get_active_subscription_count(&self) -> u32 {
        self.subscriptions.read().unwrap().len() as u32
    }

    pub async fn process_subscription(
        &self,
        subscription_id: &String,
        shared_buffer: SharedArrayBuffer,
        _requests: Vec<Request>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<(Pipeline, FxHashMap<String, Vec<Filter>>)> {
        // Acquire concurrency permit (released automatically when this fn returns)
        let _permit = self.acquire_permit().await;

        // Create pipeline based on config
        let mut pipeline = self.build_pipeline(config.pipeline(), subscription_id.clone())?;

        let (network_requests, events) =
            match self.cache_processor.process_local_requests(_requests).await {
                Ok((network_requests, events)) => (network_requests, events),
                Err(e) => {
                    error!(
                        "Failed to process local requests for subscription {}: {}",
                        subscription_id, e
                    );
                    return Err(NostrError::Other(format!(
                        "Failed to process local requests: {}",
                        e
                    )));
                }
            };

        // Process cached events through cache-capable pipes first, then write originals
        if !events.is_empty() {
            for event_batch in events {
                let cache_outputs = pipeline.process_cached_batch(&event_batch).await?;
                for out in cache_outputs {
                    match SharedBufferManager::write_to_buffer(&shared_buffer, &out).await {
                        Ok(true) => {
                            // Buffer is full: signal EOCE and return early.
                            SharedBufferManager::send_eoce(&shared_buffer).await;
                            post_worker_message(&JsValue::from_str(subscription_id));
                            // return Ok((pipeline, FxHashMap::default()));
                        }
                        Ok(false) => {
                            // Written successfully, continue
                        }
                        Err(_) => {
                            // Malformed/invalid state; keep behavior minimal (ignore and continue)
                            // You could log or handle differently if desired.
                        }
                    }
                }
            }
        }

        SharedBufferManager::send_eoce(&shared_buffer).await;

        post_worker_message(&JsValue::from_str(subscription_id));

        let relay_filters = self.group_requests_by_relay(&network_requests)?;

        Ok((pipeline, relay_filters))
    }

    fn group_requests_by_relay(
        &self,
        requests: &Vec<Request>,
    ) -> Result<FxHashMap<String, Vec<Filter>>> {
        let mut relay_filters_map: FxHashMap<String, Vec<Filter>> = FxHashMap::default();

        for request in requests {
            let relays = self.get_request_relays(request)?;
            // Convert the request to a filter
            let filter = request.to_filter()?;

            // Add the filter to each relay in the request
            for relay_url in relays {
                if let Err(e) = validate_relay_url(&relay_url) {
                    warn!("Invalid relay URL {}: {}, skipping", relay_url, e);
                    continue;
                }
                relay_filters_map
                    .entry(normalize_relay_url(&relay_url))
                    .or_insert_with(Vec::new)
                    .push(filter.clone());
            }
        }

        Ok(relay_filters_map)
    }

    fn get_request_relays(&self, request: &Request) -> Result<Vec<String>> {
        let filter = request.to_filter()?;
        if request.relays.is_empty() {
            let pubkey = match filter.authors.as_ref() {
                Some(authors) => {
                    if !authors.is_empty() {
                        authors.iter().next().unwrap().to_string()
                    } else {
                        String::new()
                    }
                }
                None => String::new(),
            };

            let kind = match filter.kinds.as_ref() {
                Some(kinds) => {
                    if !kinds.is_empty() {
                        *kinds.iter().next().unwrap()
                    } else {
                        0
                    }
                }
                None => 0,
            };

            let relays = self.database.find_relay_candidates(kind, &pubkey, &false);

            // Limit to maximum of 8 relays
            let relays_to_add: Vec<String> = relays.into_iter().take(8).collect();

            return Ok(relays_to_add);
        }

        Ok(request.relays.clone())
    }

    fn build_pipeline(
        &self,
        pipeline_config: Option<fb::PipelineConfig<'_>>,
        subscription_id: String,
    ) -> Result<Pipeline> {
        match pipeline_config {
            Some(config) => {
                let mut pipes: Vec<PipeType> = Vec::new();
                for pipe_config in config.pipes() {
                    let config_type = pipe_config.config_type();
                    let pipe = match config_type {
                        fb::PipeConfig::ParsePipeConfig => {
                            PipeType::Parse(ParsePipe::new(self.parser.clone()))
                        }
                        fb::PipeConfig::SaveToDbPipeConfig => {
                            PipeType::SaveToDb(SaveToDbPipe::new(self.database.clone()))
                        }
                        fb::PipeConfig::SerializeEventsPipeConfig => PipeType::SerializeEvents(
                            SerializeEventsPipe::new(subscription_id.clone()),
                        ),
                        fb::PipeConfig::ProofVerificationPipeConfig => {
                            let config = pipe_config
                                .config_as_proof_verification_pipe_config()
                                .unwrap();
                            // max_proofs: usize, check_interval_secs: u64
                            let max_proofs = config.max_proofs() as usize;

                            PipeType::ProofVerification(ProofVerificationPipe::new(max_proofs))
                        }
                        fb::PipeConfig::CounterPipeConfig => {
                            let config = pipe_config.config_as_counter_pipe_config().unwrap();

                            let kinds: Vec<u16> = config.kinds().iter().map(|k| k as u16).collect();

                            let pubkey = config.pubkey();

                            let pubkey = config.pubkey().to_string();

                            PipeType::Counter(CounterPipe::new(kinds, pubkey))
                        }
                        fb::PipeConfig::KindFilterPipeConfig => {
                            let config = pipe_config.config_as_kind_filter_pipe_config().unwrap();
                            let kinds: Vec<u16> = config.kinds().iter().map(|k| k as u16).collect();
                            PipeType::KindFilter(KindFilterPipe::new(kinds))
                        }
                        fb::PipeConfig::NpubLimiterPipeConfig => {
                            let config = pipe_config.config_as_npub_limiter_pipe_config().unwrap();
                            let kind = config.kind();
                            let limit_per_npub = config.limit_per_npub();
                            let max_total_npubs = config.max_total_npubs();
                            PipeType::NpubLimiter(NpubLimiterPipe::new(kind, limit_per_npub))
                        }
                        _ => {
                            return Err(NostrError::Other(format!(
                                "Unknown pipe config type: {:?}",
                                config_type
                            )))
                        }
                    };
                    pipes.push(pipe);
                }
                if pipes.is_empty() {
                    Pipeline::default(self.parser.clone(), self.database.clone(), subscription_id)
                } else {
                    Pipeline::new(pipes, subscription_id)
                }
            }
            None => {
                // Use default pipeline
                Pipeline::default(self.parser.clone(), self.database.clone(), subscription_id)
            }
        }
    }
}
