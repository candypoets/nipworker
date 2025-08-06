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
use anyhow::Result;
use js_sys::{SharedArrayBuffer, Uint8Array};
use rmp_serde;
use rustc_hash::FxHashMap;
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
            .insert(subscription_id.clone(), shared_buffer);

        // Spawn subscription processing task
        self.process_subscription(&subscription_id, requests, config)
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
                    if let Some(output) = pipeline.process(pipeline_event).await? {
                        self.write_to_shared_buffer(subscription_id, &output).await;
                    }
                }
            }
        }

        let _ = self.send_eoce(&subscription_id).await;

        // Only process network requests if there are any
        if !network_requests.is_empty() {
            let relay_filters = self.group_requests_by_relay(network_requests.clone())?;
            let subscription_handle = self
                .connection_registry
                .subscribe(subscription_id.clone(), relay_filters.clone())
                .await?;

            // Move pipeline into spawn_local task
            let _cache_processor = self.cache_processor.clone();
            let shared_buffer = {
                let subscriptions = self.subscriptions.read().unwrap();
                subscriptions.get(subscription_id.as_str()).cloned()
            };

            let relay_urls = subscription_handle.relay_urls();
            let sub_id = subscription_id.clone();
            let total_connections = relay_urls.len() as i32;
            let mut remaining_connections = total_connections;
            let close_on_eose = config.close_on_eose;

            let mut relay_eose_status: FxHashMap<String, bool> = FxHashMap::default();
            for relay_url in relay_urls {
                relay_eose_status.insert(relay_url.clone(), false);
            }

            // Process events from the subscription handle
            spawn_local(async move {
                let mut handle = subscription_handle;
                let mut pipeline = pipeline; // Move pipeline into task

                while let Some(event) = handle.next_event().await {
                    match event.event_type {
                        NetworkEventType::Event => {
                            debug!("Received event from relay: {:?}", event.relay);
                            if let Some(raw_event) = event.event {
                                // Process raw event through pipeline
                                let pipeline_event =
                                    PipelineEvent::from_raw(raw_event, event.relay);
                                match pipeline.process(pipeline_event).await {
                                    Ok(Some(output)) => {
                                        // Send output to buffer
                                        if let Some(ref buffer) = shared_buffer {
                                            Self::write_to_buffer(buffer, &sub_id, &output).await;
                                        }
                                    }
                                    Ok(None) => {
                                        // Event was dropped by pipeline
                                    }
                                    Err(e) => {
                                        warn!("Pipeline processing failed: {}", e);
                                    }
                                }
                            }
                        }
                        NetworkEventType::EOSE => {
                            if let Some(relay) = event.relay.clone() {
                                relay_eose_status.insert(relay, true);
                            }
                            remaining_connections -= 1;
                            // Send End of Stored Events notification
                            debug!(
                                "Received EOSE from relay {:?} for subscription {} (Remaining: {}/{})",
                                event.relay, sub_id, remaining_connections, total_connections
                            );
                            if let Some(ref buffer) = shared_buffer {
                                Self::send_eose(
                                    buffer,
                                    &sub_id,
                                    EOSE {
                                        total_connections: total_connections,
                                        remaining_connections: remaining_connections,
                                    },
                                )
                                .await;
                            }

                            // Check if we should close the subscription when all EOSE received
                            if close_on_eose && remaining_connections == 0 {
                                info!(
                                    "All relays sent EOSE, closing subscription {} as requested",
                                    sub_id
                                );
                                // Subscription will naturally close when handle completes
                            }
                        }
                        NetworkEventType::Error => {
                            warn!("Received error event from network: {:?}", event);
                        }
                    }
                }
                info!(
                    "🔚 Subscription handle completed for subscription {}",
                    sub_id
                );
            });
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

    async fn send_eose(shared_buffer: &SharedArrayBuffer, subscription_id: &str, eose: EOSE) {
        let message = crate::WorkerToMainMessage::Eose {
            subscription_id: subscription_id.to_string(),
            data: eose,
        };

        let data = match rmp_serde::to_vec_named(&message) {
            Ok(msgpack) => msgpack,
            Err(e) => {
                error!(
                    "Failed to serialize EOSE for subscription {}: {}",
                    subscription_id, e
                );
                return;
            }
        };

        let _ = Self::write_to_buffer(shared_buffer, subscription_id, &data).await;
    }

    async fn send_eoce(&self, subscription_id: &str) {
        let message = crate::WorkerToMainMessage::Eoce {
            subscription_id: subscription_id.to_string(),
        };

        let data = match rmp_serde::to_vec_named(&message) {
            Ok(msgpack) => msgpack,
            Err(e) => {
                error!(
                    "Failed to serialize EOCE for subscription {}: {}",
                    subscription_id, e
                );
                return;
            }
        };

        self.write_to_shared_buffer(subscription_id, &data).await;
    }

    async fn write_to_shared_buffer(&self, subscription_id: &str, data: &[u8]) {
        let subscriptions = self.subscriptions.read().unwrap();
        if let Some(shared_buffer) = subscriptions.get(subscription_id) {
            Self::write_to_buffer(shared_buffer, subscription_id, data).await;
        } else {
            warn!(
                "No SharedArrayBuffer found for subscription: {}, dropping message",
                subscription_id
            );
        }
    }

    fn has_buffer_full_marker(
        buffer_uint8: &Uint8Array,
        current_write_pos: usize,
        buffer_length: usize,
    ) -> bool {
        // Check if the last written entry is already a buffer full marker
        if current_write_pos < 5 {
            return false;
        }

        // Read the length of the previous entry (4 bytes before current position)
        let prev_length_pos = current_write_pos - 5; // -5 because marker is 1 byte + 4 byte length
        let prev_length_subarray =
            buffer_uint8.subarray(prev_length_pos as u32, (prev_length_pos + 4) as u32);
        let mut prev_length_bytes = vec![0u8; 4];
        prev_length_subarray.copy_to(&mut prev_length_bytes[..]);

        let mut prev_length_array = [0u8; 4];
        prev_length_array.copy_from_slice(&prev_length_bytes);
        let prev_length = u32::from_le_bytes(prev_length_array);

        // If the previous entry has length 1, check if it's the buffer full marker (0xFF)
        if prev_length == 1 {
            let prev_data_pos = prev_length_pos + 4;
            if prev_data_pos < buffer_length {
                let prev_data_subarray =
                    buffer_uint8.subarray(prev_data_pos as u32, (prev_data_pos + 1) as u32);
                let mut prev_data_bytes = vec![0u8; 1];
                prev_data_subarray.copy_to(&mut prev_data_bytes[..]);

                return prev_data_bytes[0] == 0xFF;
            }
        }

        false
    }

    async fn write_to_buffer(
        shared_buffer: &SharedArrayBuffer,
        subscription_id: &str,
        data: &[u8],
    ) {
        // Add safety checks for data size
        if data.len() > 1024 * 1024 {
            // 1MB limit
            warn!(
                "Data too large for SharedArrayBuffer: {} bytes for subscription {}",
                data.len(),
                subscription_id
            );
            warn!("Dropping message due to size limit");
            return;
        }

        // Get the buffer as Uint8Array for manipulation
        let buffer_uint8 = Uint8Array::new(shared_buffer);
        let buffer_length = buffer_uint8.length() as usize;

        // Read current write position from header (first 4 bytes, little endian)
        let header_subarray = buffer_uint8.subarray(0, 4);
        let mut header_bytes = vec![0u8; 4];
        header_subarray.copy_to(&mut header_bytes[..]);

        let mut header_array = [0u8; 4];
        header_array.copy_from_slice(&header_bytes);
        let current_write_pos = u32::from_le_bytes(header_array) as usize;

        // Safety check for current write position
        if current_write_pos >= buffer_length {
            warn!(
                "Invalid write position {} >= buffer length {} for subscription {}",
                current_write_pos, buffer_length, subscription_id
            );
            warn!("Dropping message due to invalid write position");
            return;
        }

        // Check if we have enough space (4 bytes write position header + 4 bytes length prefix + data)
        let new_write_pos = current_write_pos + 4 + data.len(); // +4 for length prefix
        if new_write_pos > buffer_length {
            // Check if the last written entry is already a buffer full marker
            if Self::has_buffer_full_marker(&buffer_uint8, current_write_pos, buffer_length) {
                warn!(
                    "Buffer full for subscription {}, but marker already exists",
                    subscription_id
                );
                return;
            }
            // Write minimal "buffer full" marker: length=1, data=0xFF
            if current_write_pos + 5 <= buffer_length {
                // 4 bytes length + 1 byte marker
                let length_prefix = 1u32.to_le_bytes(); // Length = 1
                let length_prefix_uint8 = Uint8Array::from(&length_prefix[..]);
                buffer_uint8.set(&length_prefix_uint8, current_write_pos as u32);

                let marker = [0xFF]; // Single byte marker for "buffer full"
                let marker_uint8 = Uint8Array::from(&marker[..]);
                buffer_uint8.set(&marker_uint8, (current_write_pos + 4) as u32);

                // Update write position
                let new_pos = current_write_pos + 5;
                let new_header = (new_pos as u32).to_le_bytes();
                let new_header_uint8 = Uint8Array::from(&new_header[..]);
                buffer_uint8.set(&new_header_uint8, 0);

                warn!(
                    "Buffer full for subscription {}, wrote 1-byte marker",
                    subscription_id
                );
            } else {
                warn!(
                    "Buffer completely full for subscription {}, cannot even write marker",
                    subscription_id
                );
            }
            return;
        }

        // Write the length prefix (4 bytes, little endian) at current write position
        let length_prefix = (data.len() as u32).to_le_bytes();
        let length_prefix_uint8 = Uint8Array::from(&length_prefix[..]);
        buffer_uint8.set(&length_prefix_uint8, current_write_pos as u32);

        // Write the actual data after the length prefix
        let data_uint8 = Uint8Array::from(data);
        buffer_uint8.set(&data_uint8, (current_write_pos + 4) as u32);

        // Update the header with new write position (little endian)
        let new_header = (new_write_pos as u32).to_le_bytes();
        let new_header_uint8 = Uint8Array::from(&new_header[..]);
        buffer_uint8.set(&new_header_uint8, 0);

        debug!(
            "Wrote {} bytes (+ 4 byte length prefix) to SharedArrayBuffer for subscription: {} (pos: {} -> {}) and notified waiters",
            data.len(),
            subscription_id,
            current_write_pos,
            new_write_pos
        );
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
                            let update_interval = params
                                .and_then(|p| p.get("updateInterval"))
                                .and_then(|v| v.as_u64())
                                .unwrap_or(100);
                            PipeType::Counter(CounterPipe::new(kinds, update_interval))
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
