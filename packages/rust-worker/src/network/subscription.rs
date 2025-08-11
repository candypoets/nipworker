use super::*;
use crate::db::NostrDB;
use crate::network::cache_processor::CacheProcessor;
use crate::network::interfaces::CacheProcessor as CacheProcessorTrait;
use crate::parser::Parser;
use crate::pipeline::pipes::*;
use crate::pipeline::{PipeType, Pipeline, PipelineEvent};
use crate::relays::utils::{normalize_relay_url, validate_relay_url};
use crate::types::network::Request;
use crate::types::thread::{PipelineConfig, SubscriptionConfig};
use crate::types::*;
use crate::utils::buffer::SharedBufferManager;
use anyhow::Result;
use futures::lock::Mutex;
use js_sys::{SharedArrayBuffer, Uint8Array};
use rmp_serde;
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info, warn};

use wasm_bindgen_futures::spawn_local;

pub struct SubscriptionManager {
    database: Arc<NostrDB>,
    parser: Arc<Parser>,
    subscriptions: Arc<RwLock<FxHashMap<String, SharedArrayBuffer>>>,
    cache_processor: Arc<CacheProcessor>,
    connection_registry: ConnectionRegistry,
    relay_hints: FxHashMap<String, Vec<String>>,
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
            connection_registry: ConnectionRegistry::new(),
        }
    }

    pub async fn open_subscription(
        &self,
        subscription_id: String,
        requests: Vec<Request>,
        shared_buffer: SharedArrayBuffer,
        config: Option<SubscriptionConfig>,
    ) -> Result<()> {
        let config = config.unwrap_or_default();

        info!(
            "Opening subscription: {} with {} requests (closeOnEOSE: {}, cacheFirst: {}){}",
            subscription_id,
            requests.len(),
            config.close_on_eose,
            config.cache_first,
            if requests.len() == 1 {
                format!(" with filter: {:?}", requests[0].tags)
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

        // Spawn subscription processing task
        self.process_subscription(&subscription_id, requests, shared_buffer.clone(), config)
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
        subscription_id: &String,
        _requests: Vec<Request>,
        shared_buffer: SharedArrayBuffer,
        config: SubscriptionConfig,
    ) -> Result<()> {
        debug!("Processing subscription: {}", subscription_id);

        // Create pipeline based on config
        let mut pipeline = self.build_pipeline(config.pipeline.clone(), subscription_id.clone())?;

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
                return Err(anyhow::anyhow!("Failed to process local requests: {}", e));
            }
        };

        // Process cached events through pipeline
        if !events.is_empty() {
            for event_batch in events {
                for parsed_event in event_batch {
                    let pipeline_event = PipelineEvent::from_parsed(parsed_event);
                    if let Some(output) = pipeline.process_cached_event(pipeline_event).await? {
                        SharedBufferManager::write_to_buffer(&shared_buffer, &output).await;
                    }
                }
            }
        }

        let _ = SharedBufferManager::send_eoce(&shared_buffer).await;

        // Only process network requests if there are any
        if !network_requests.is_empty() {
            let relay_filters = self.group_requests_by_relay(network_requests.clone())?;
            // Kick off direct subscription setup — writes to SAB inside connection loop
            self.connection_registry
                .subscribe(
                    subscription_id.clone(),
                    relay_filters,
                    Rc::new(Mutex::new(pipeline)),
                    Rc::new(shared_buffer.clone()),
                    config.close_on_eose,
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
        requests: Vec<Request>,
    ) -> Result<FxHashMap<String, Vec<Filter>>, anyhow::Error> {
        let mut relay_filters_map: FxHashMap<String, Vec<Filter>> = FxHashMap::default();

        for mut request in requests {
            request = self.set_request_relay(request)?;
            // Convert the request to a filter
            let filter = request.to_filter()?;

            // Add the filter to each relay in the request
            for relay_url in request.relays {
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

    fn set_request_relay(&self, mut request: Request) -> Result<Request> {
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
                        kinds.iter().next().unwrap().as_u64()
                    } else {
                        0
                    }
                }
                None => 0,
            };

            let relays = self.database.find_relay_candidates(kind, &pubkey, &false);

            info!(
                "No relays specified, found {} relay candidates",
                relays.len()
            );

            // Limit to maximum of 8 relays
            let relays_to_add: Vec<String> = relays.into_iter().take(8).collect();

            request.relays.extend(relays_to_add);
        }

        Ok(request)
    }

    fn build_pipeline(
        &self,
        pipeline_config: Option<PipelineConfig>,
        subscription_id: String,
    ) -> Result<Pipeline> {
        match pipeline_config {
            Some(config) => {
                let mut pipes: Vec<PipeType> = Vec::new();

                for pipe_config in config.pipes {
                    let pipe = match pipe_config.name.as_str() {
                        "deduplication" => {
                            let max_size = pipe_config
                                .params
                                .as_ref()
                                .and_then(|p| p.get("maxSize").or_else(|| p.get("max_size")))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(10000)
                                as usize;
                            PipeType::Deduplication(DeduplicationPipe::new(max_size))
                        }
                        "parse" => PipeType::Parse(ParsePipe::new(self.parser.clone())),
                        "saveToDb" | "save_to_db" => {
                            PipeType::SaveToDb(SaveToDbPipe::new(self.database.clone()))
                        }
                        "serializeEvents" | "serialize_events" => PipeType::SerializeEvents(
                            SerializeEventsPipe::new(subscription_id.clone()),
                        ),
                        "proofVerification" => {
                            let params = pipe_config.params.as_ref();
                            // max_proofs: usize, check_interval_secs: u64
                            let max_proofs = params
                                .and_then(|p| p.get("maxProofs"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(100)
                                as usize;
                            let check_interval = params
                                .and_then(|p| p.get("checkIntervalSecs"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1);

                            info!("Creating ProofVerificationPipe with max_proofs: {}, check_interval: {}s", max_proofs, check_interval);

                            PipeType::ProofVerification(ProofVerificationPipe::new(max_proofs))
                        }
                        "counter" => {
                            let params = pipe_config.params.as_ref();
                            let kinds = params
                                .and_then(|p| p.get("kinds"))
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
                                .unwrap_or_else(|| vec![1]); // Default to kind 1 (text notes)

                            let pubkey = params
                                .and_then(|p| p.get("pubkey"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();

                            PipeType::Counter(CounterPipe::new(kinds, pubkey))
                        }
                        "kindFilter" | "kind_filter" => {
                            let kinds = pipe_config
                                .params
                                .as_ref()
                                .and_then(|p| p.get("kinds"))
                                .and_then(|v| v.as_array())
                                .map(|arr| arr.iter().filter_map(|v| v.as_u64()).collect())
                                .unwrap_or_default();
                            PipeType::KindFilter(KindFilterPipe::new(kinds))
                        }
                        "npubLimiter" | "npub_limiter" => {
                            let params = pipe_config.params.as_ref();
                            let kind = params
                                .and_then(|p| p.get("kind"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(1); // Default to kind 1 (text notes)
                            let limit_per_npub = params
                                .and_then(|p| p.get("limitPerNpub"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(5)
                                as usize;
                            let max_total_npubs = params
                                .and_then(|p| p.get("maxTotalNpubs"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(100)
                                as usize;
                            PipeType::NpubLimiter(NpubLimiterPipe::new(
                                kind,
                                limit_per_npub,
                                max_total_npubs,
                            ))
                        }
                        _ => return Err(anyhow::anyhow!("Unknown pipe: {}", pipe_config.name)),
                    };
                    pipes.push(pipe);
                }

                Pipeline::new(pipes, subscription_id)
            }
            None => {
                // Use default pipeline
                Pipeline::default(self.parser.clone(), self.database.clone(), subscription_id)
            }
        }
    }
}
