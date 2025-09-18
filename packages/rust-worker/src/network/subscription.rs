use super::*;
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
use futures::lock::Mutex;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;

type Result<T> = std::result::Result<T, NostrError>;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info, warn};
use wasm_bindgen::JsValue;

pub struct SubscriptionManager {
    database: Arc<NostrDB>,
    parser: Arc<Parser>,
    subscriptions: Arc<RwLock<FxHashMap<String, SharedArrayBuffer>>>,
    cache_processor: Arc<CacheProcessor>,
    connection_registry: Arc<ConnectionRegistry>,
    relay_hints: FxHashMap<String, Vec<String>>,
}

impl SubscriptionManager {
    pub fn new(
        database: Arc<NostrDB>,
        connection_registry: Arc<ConnectionRegistry>,
        parser: Arc<Parser>,
    ) -> Self {
        let cache_processor = Arc::new(CacheProcessor::new(database.clone(), parser.clone()));

        Self {
            database: database.clone(),
            connection_registry,
            parser,
            subscriptions: Arc::new(RwLock::new(FxHashMap::default())),
            relay_hints: FxHashMap::default(),
            cache_processor,
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        shared_buffer: SharedArrayBuffer,
        requests: &Vec<fb::Request<'_>>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<()> {
        info!(
            "Opening subscription: {} with {} requests (closeOnEOSE: {}, cacheFirst: {}){}",
            subscription_id,
            requests.len(),
            config.close_on_eose(),
            config.cache_first(),
            if requests.len() == 1 {
                format!(" with filter: {:?}", requests[0].tags())
            } else {
                String::new()
            }
        );

        if self
            .subscriptions
            .read()
            .unwrap()
            .contains_key(&subscription_id)
        {
            debug!("Subscription {} already exists", subscription_id);
            return Ok(());
        }

        self.subscriptions
            .write()
            .unwrap()
            .insert(subscription_id.clone(), shared_buffer.clone());

        let parsed_requests: Vec<Request> = requests
            .iter()
            .map(|request| Request::from_flatbuffer(request))
            .collect();

        // Spawn subscription processing task
        self.process_subscription(
            shared_buffer.clone(),
            &subscription_id,
            parsed_requests,
            config,
        )
        .await?;

        debug!("Subscription {} opened successfully", subscription_id);
        Ok(())
    }

    pub async fn close_subscription(&self, subscription_id: &String) -> Result<()> {
        info!("Closing subscription: {}", subscription_id);

        self.connection_registry
            .close_subscription(&subscription_id)
            .await?;

        // drop the reference to the sharedBuffer
        self.subscriptions.write().unwrap().remove(subscription_id);

        debug!(
            "Subscription {} closed (SharedArrayBuffer retained)",
            subscription_id
        );

        Ok(())
    }

    pub async fn get_active_subscription_count(&self) -> u32 {
        self.subscriptions.read().unwrap().len() as u32
    }

    async fn process_subscription(
        &self,
        shared_buffer: SharedArrayBuffer,
        subscription_id: &String,
        _requests: Vec<Request>,
        config: &fb::SubscriptionConfig<'_>,
    ) -> Result<()> {
        debug!("Processing subscription: {}", subscription_id);

        // Create pipeline based on config
        let mut pipeline = self.build_pipeline(config.pipeline(), subscription_id.clone())?;

        let (network_requests, events) = match self
            .cache_processor
            .process_local_requests(_requests, 3)
            .await
        {
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
                    SharedBufferManager::write_to_buffer(&shared_buffer, &out).await;
                }
            }
        }

        info!("Sending eoce event {}", subscription_id);
        SharedBufferManager::send_eoce(&shared_buffer).await;

        post_worker_message(&JsValue::from_str(subscription_id));

        // Only process network requests if there are any
        if !network_requests.is_empty() {
            let relay_filters = self.group_requests_by_relay(&network_requests)?;

            // Kick off direct subscription setup — writes to SAB inside connection loop
            self.connection_registry
                .subscribe(
                    subscription_id.clone(),
                    relay_filters,
                    Rc::new(Mutex::new(pipeline)),
                    Rc::new(shared_buffer.clone()),
                    config.close_on_eose(),
                )
                .await?;
        }

        // If there are no network requests, we consider the subscription complete
        if network_requests.is_empty() {
            info!(
                "Subscription {} complete (no network requests needed)",
                subscription_id
            );
        }

        Ok(())
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

            debug!(
                "No relays specified, found {} relay candidates",
                relays.len()
            );

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
