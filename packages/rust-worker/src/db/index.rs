use crate::db::storage::IndexedDbStorage;
use crate::db::types::{
    intersect_event_sets, DatabaseConfig, DatabaseError, DatabaseIndexes, DatabaseStats,
    EventStorage, ProcessedNostrEvent, QueryFilter, QueryResult,
};
use crate::network::interfaces::EventDatabase;
use crate::types::{network::Request, ParsedEvent};
use crate::utils::relay::RelayUtils;
use anyhow::Result;
use async_trait::async_trait;
use gloo_timers::future::TimeoutFuture;
use instant::Instant;
use nostr::{Event, EventId, Filter, PublicKey};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info, warn};
use wasm_bindgen_futures::spawn_local;

/// Main NostrDB implementation with in-memory indexes and persistent storage
pub struct NostrDB {
    /// Database configuration
    config: DatabaseConfig,
    /// In-memory indexes for fast querying
    indexes: Arc<RwLock<DatabaseIndexes>>,
    /// Persistent storage backend
    storage: Arc<dyn EventStorage>,
    /// Initialization flag
    is_initialized: Arc<RwLock<bool>>,
    /// Buffer for events to be saved to storage
    to_save: Arc<RwLock<Vec<ParsedEvent>>>,
    /// Default relays for nostr operations
    pub default_relays: Vec<String>,
    /// Indexer relays for nostr operations
    pub indexer_relays: Vec<String>,
    /// Relay hints for pubkeys
    pub relay_hints: HashMap<String, Vec<String>>,
    /// Counter for round-robin indexer relay selection
    indexer_relay_counter: Arc<RwLock<usize>>,
    /// Counter for round-robin default relay selection
    default_relay_counter: Arc<RwLock<usize>>,
}

impl NostrDB {
    /// Create a new NostrDB instance with default IndexedDB storage
    pub fn new() -> Self {
        info!("Creating new nostr db");
        let config = DatabaseConfig::default();
        let storage = Arc::new(IndexedDbStorage::new(
            "nostr-local-relay".to_string(),
            config.clone(),
        ));

        Self {
            config,
            indexes: Arc::new(RwLock::new(DatabaseIndexes::new())),
            storage,
            is_initialized: Arc::new(RwLock::new(false)),
            to_save: Arc::new(RwLock::new(Vec::new())),
            default_relays: vec![
                "wss://relay.damus.io".to_string(),
                "wss://nos.lol".to_string(),
                "wss://relay.primal.net".to_string(),
            ],
            indexer_relays: vec![
                "wss://relay.nostr.band".to_string(),
                "wss://nostr.wine".to_string(),
            ],
            relay_hints: HashMap::new(),
            indexer_relay_counter: Arc::new(RwLock::new(0)),
            default_relay_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Create a new NostrDB instance with custom configuration and storage
    pub fn with_storage(config: DatabaseConfig, storage: Arc<dyn EventStorage>) -> Self {
        Self {
            config,
            indexes: Arc::new(RwLock::new(DatabaseIndexes::new())),
            storage,
            is_initialized: Arc::new(RwLock::new(false)),
            to_save: Arc::new(RwLock::new(Vec::new())),
            default_relays: vec![
                "wss://relay.damus.io".to_string(),
                "wss://nos.lol".to_string(),
                "wss://relay.primal.net".to_string(),
            ],
            indexer_relays: vec![
                "wss://relay.nostr.band".to_string(),
                "wss://nostr.wine".to_string(),
            ],
            relay_hints: HashMap::new(),
            indexer_relay_counter: Arc::new(RwLock::new(0)),
            default_relay_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Create a new NostrDB instance with custom relay configurations
    pub fn with_relays(default_relays: Vec<String>, indexer_relays: Vec<String>) -> Self {
        info!("Creating new nostr db with custom relays");
        let config = DatabaseConfig::default();
        let storage = Arc::new(IndexedDbStorage::new(
            "nostr-local-relay".to_string(),
            config.clone(),
        ));

        Self {
            config,
            indexes: Arc::new(RwLock::new(DatabaseIndexes::new())),
            storage,
            to_save: Arc::new(RwLock::new(Vec::new())),
            is_initialized: Arc::new(RwLock::new(false)),
            default_relays,
            indexer_relays,
            relay_hints: HashMap::new(),
            indexer_relay_counter: Arc::new(RwLock::new(0)),
            default_relay_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Create a new NostrDB instance with custom configuration, storage, and relays
    pub fn with_storage_and_relays(
        config: DatabaseConfig,
        storage: Arc<dyn EventStorage>,
        default_relays: Vec<String>,
        indexer_relays: Vec<String>,
    ) -> Self {
        Self {
            config,
            indexes: Arc::new(RwLock::new(DatabaseIndexes::new())),
            storage,
            is_initialized: Arc::new(RwLock::new(false)),
            to_save: Arc::new(RwLock::new(Vec::new())),
            default_relays,
            indexer_relays,
            relay_hints: HashMap::new(),
            indexer_relay_counter: Arc::new(RwLock::new(0)),
            default_relay_counter: Arc::new(RwLock::new(0)),
        }
    }

    /// Initialize the database by loading events from persistent storage
    pub async fn initialize(&self) -> Result<(), DatabaseError> {
        info!("Initializing NostrDB...");

        let mut is_init = self
            .is_initialized
            .write()
            .map_err(|_| DatabaseError::LockError)?;
        if *is_init {
            debug!("Database already initialized");
            return Ok(());
        }

        // Clear existing indexes
        {
            let mut indexes = self.indexes.write().map_err(|_| DatabaseError::LockError)?;
            indexes.clear();
        }

        // Load events from storage
        let events = self.storage.load_events().await?;

        if !events.is_empty() {
            info!("Loading {} events from persistent storage", events.len());
            self.build_indexes_from_events(events)?;
        }

        *is_init = true;
        let event_count = {
            let indexes = self.indexes.read().map_err(|_| DatabaseError::LockError)?;
            indexes.events_by_id.len()
        };
        info!(
            "NostrDB initialization complete with {} events in cache",
            event_count
        );

        // Start the periodic save task
        self.start_periodic_save_task();

        Ok(())
    }

    /// Check if the database is initialized
    pub fn is_initialized(&self) -> bool {
        *self
            .is_initialized
            .read()
            .unwrap_or_else(|_| panic!("Lock poisoned"))
    }

    /// Build indexes from a collection of events
    fn build_indexes_from_events(
        &self,
        events: Vec<ProcessedNostrEvent>,
    ) -> Result<(), DatabaseError> {
        let start_time = Instant::now();
        let mut indexes = self.indexes.write().map_err(|_| DatabaseError::LockError)?;

        // Pre-allocate maps based on event count for better performance
        let event_count = events.len();
        let mut pubkey_frequency = HashMap::new();
        let mut kind_frequency = HashMap::new();

        // First pass: count frequencies for optimal map sizing
        for event in &events {
            *pubkey_frequency.entry(event.pubkey()).or_insert(0) += 1;
            *kind_frequency.entry(event.kind()).or_insert(0) += 1;
        }

        // Pre-allocate maps for high-frequency items
        for (pubkey, count) in pubkey_frequency {
            if count > 5 {
                indexes
                    .events_by_pubkey
                    .insert(pubkey, HashSet::with_capacity(count));
            }
        }

        for (kind, count) in kind_frequency {
            indexes
                .events_by_kind
                .insert(kind, HashSet::with_capacity(count));
        }

        // Second pass: build all indexes
        for event in events {
            self.index_event(&mut indexes, event);
        }

        let duration = start_time.elapsed();
        info!(
            "Built indexes for {} events in {:?} (memory: ~{} bytes)",
            event_count,
            duration,
            indexes.estimate_memory_usage()
        );

        Ok(())
    }

    /// Add an event to all relevant indexes
    fn index_event(&self, indexes: &mut DatabaseIndexes, event: ProcessedNostrEvent) {
        let event_id = event.id();

        // Primary index
        indexes.events_by_id.insert(event_id.clone(), event.clone());

        // Kind index
        indexes
            .events_by_kind
            .entry(event.kind())
            .or_insert_with(HashSet::new)
            .insert(event_id.clone());

        // Pubkey index
        indexes
            .events_by_pubkey
            .entry(event.pubkey())
            .or_insert_with(HashSet::new)
            .insert(event_id.clone());

        // Tag indexes
        for e_tag in &event.e_tags {
            indexes
                .events_by_e_tag
                .entry(e_tag.clone())
                .or_insert_with(HashSet::new)
                .insert(event_id.clone());
        }

        for p_tag in &event.p_tags {
            indexes
                .events_by_p_tag
                .entry(p_tag.clone())
                .or_insert_with(HashSet::new)
                .insert(event_id.clone());
        }

        for a_tag in &event.a_tags {
            indexes
                .events_by_a_tag
                .entry(a_tag.clone())
                .or_insert_with(HashSet::new)
                .insert(event_id.clone());
        }

        for d_tag in &event.d_tags {
            indexes
                .events_by_d_tag
                .entry(d_tag.clone())
                .or_insert_with(HashSet::new)
                .insert(event_id.clone());
        }

        // Special handling for profiles (kind 0)
        if event.kind() == 0 {
            indexes
                .profiles_by_pubkey
                .insert(event.pubkey(), event.clone());
        }

        // Special handling for relay lists (kind 10002)
        if event.kind() == 10002 {
            indexes.relays_by_pubkey.insert(event.pubkey(), event);
        }
    }

    /// Query events using the internal filter format
    pub fn query_events_internal(&self, filter: QueryFilter) -> Result<QueryResult, DatabaseError> {
        let start_time = Instant::now();
        let indexes = self.indexes.read().map_err(|_| DatabaseError::LockError)?;

        // Start with candidate sets from indexed fields
        let mut candidate_sets = Vec::new();
        let mut use_full_scan = true;

        // Filter by IDs (most specific)
        if let Some(ids) = &filter.ids {
            let mut id_set = HashSet::new();
            for id in ids {
                let id_hex = id.to_hex();
                if indexes.events_by_id.contains_key(&id_hex) {
                    id_set.insert(id_hex);
                }
            }
            candidate_sets.push(id_set);
            use_full_scan = false;
        }

        // Filter by kinds
        if let Some(kinds) = &filter.kinds {
            let mut kind_events = HashSet::new();
            for kind in kinds {
                let kind_u64 = kind.as_u64();
                if let Some(event_ids) = indexes.events_by_kind.get(&kind_u64) {
                    kind_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(kind_events);
            use_full_scan = false;
        }

        // Filter by authors
        if let Some(authors) = &filter.authors {
            let mut author_events = HashSet::new();
            for author in authors {
                let author_hex = author.to_hex();
                if let Some(event_ids) = indexes.events_by_pubkey.get(&author_hex) {
                    author_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(author_events);
            use_full_scan = false;
        }

        // Filter by e_tags
        if let Some(e_tags) = &filter.e_tags {
            let mut tag_events = HashSet::new();
            for tag in e_tags {
                if let Some(event_ids) = indexes.events_by_e_tag.get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Filter by p_tags
        if let Some(p_tags) = &filter.p_tags {
            let mut tag_events = HashSet::new();
            for tag in p_tags {
                if let Some(event_ids) = indexes.events_by_p_tag.get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Filter by a_tags
        if let Some(a_tags) = &filter.a_tags {
            let mut tag_events = HashSet::new();
            for tag in a_tags {
                if let Some(event_ids) = indexes.events_by_a_tag.get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Filter by d_tags
        if let Some(d_tags) = &filter.d_tags {
            let mut tag_events = HashSet::new();
            for tag in d_tags {
                if let Some(event_ids) = indexes.events_by_d_tag.get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Get final candidate set
        let candidate_ids = if use_full_scan {
            // No indexed filters, scan all events
            indexes.events_by_id.keys().cloned().collect()
        } else if candidate_sets.is_empty() {
            HashSet::new()
        } else {
            // Intersect all candidate sets
            let candidate_refs: Vec<&HashSet<String>> = candidate_sets.iter().collect();
            intersect_event_sets(candidate_refs)
        };

        // Apply non-indexed filters and collect results
        let mut results = Vec::new();
        let search_lower = filter.search.as_ref().map(|s| s.to_lowercase());

        for event_id in candidate_ids {
            if let Some(event) = indexes.events_by_id.get(&event_id) {
                // Time range filters
                if let Some(since) = filter.since {
                    if event.created_at() < since {
                        continue;
                    }
                }

                if let Some(until) = filter.until {
                    if event.created_at() > until {
                        continue;
                    }
                }

                // Search filter
                if let Some(search) = &search_lower {
                    if !event.event.content.to_lowercase().contains(search) {
                        continue;
                    }
                }

                results.push(event.to_parsed_event());
            }
        }

        let total_found = results.len();

        // Sort by created_at (newest first)
        results.sort_by(|a, b| b.event.created_at.cmp(&a.event.created_at));

        // Apply limit
        let has_more = if let Some(limit) = filter.limit {
            let limited = results.len() > limit;
            if limited {
                results.truncate(limit);
            }
            limited
        } else {
            false
        };

        let query_time = start_time.elapsed();

        if self.config.debug_logging {
            debug!(
                "Query completed: found {} events, returned {} events in {:?}",
                total_found,
                results.len(),
                query_time
            );
        }

        Ok(QueryResult {
            events: results,
            total_found,
            has_more,
            query_time_ms: query_time.as_millis() as u64,
        })
    }

    /// Get a single event by ID
    pub fn get_event(&self, id: &EventId) -> Option<ParsedEvent> {
        let indexes = self.indexes.read().ok()?;
        indexes
            .events_by_id
            .get(&id.to_hex())
            .map(|e| e.to_parsed_event())
    }

    /// Check if an event exists
    pub fn has_event(&self, id: &EventId) -> bool {
        let indexes = self
            .indexes
            .read()
            .unwrap_or_else(|_| panic!("Lock poisoned"));
        indexes.events_by_id.contains_key(&id.to_hex())
    }

    /// Get a profile for a given pubkey
    pub fn get_profile(&self, pubkey: &PublicKey) -> Option<ParsedEvent> {
        let indexes = self.indexes.read().ok()?;
        indexes
            .profiles_by_pubkey
            .get(&pubkey.to_hex())
            .map(|e| e.to_parsed_event())
    }

    pub fn get_read_relays(&self, pubkey: &str) -> Option<Vec<String>> {
        let indexes = self.indexes.read().ok()?;
        let parsed_event = indexes
            .relays_by_pubkey
            .get(pubkey)
            .map(|e| e.to_parsed_event());

        match parsed_event {
            Some(parsed_event) => {
                if let Some(parsed_data) = &parsed_event.parsed {
                    if let Ok(kind10002_parsed) = serde_json::from_value::<
                        crate::parser::Kind10002Parsed,
                    >(parsed_data.clone())
                    {
                        return Some(
                            kind10002_parsed
                                .into_iter()
                                .filter(|relay| relay.read)
                                .map(|relay| relay.url)
                                .collect::<Vec<_>>(),
                        );
                    } else {
                        return Some(Vec::new());
                    }
                } else {
                    return Some(Vec::new());
                }
            }
            None => None,
        }
    }

    pub fn get_write_relays(&self, pubkey: &str) -> Option<Vec<String>> {
        let indexes = self.indexes.read().ok()?;
        let parsed_event = indexes
            .relays_by_pubkey
            .get(pubkey)
            .map(|e| e.to_parsed_event());

        match parsed_event {
            Some(parsed_event) => {
                if let Some(parsed_data) = &parsed_event.parsed {
                    if let Ok(kind10002_parsed) = serde_json::from_value::<
                        crate::parser::Kind10002Parsed,
                    >(parsed_data.clone())
                    {
                        return Some(
                            kind10002_parsed
                                .into_iter()
                                .filter(|relay| relay.write)
                                .map(|relay| relay.url)
                                .collect::<Vec<_>>(),
                        );
                    } else {
                        return Some(Vec::new());
                    }
                } else {
                    return Some(Vec::new());
                }
            }
            None => None,
        }
    }

    pub fn get_relay_hint(&mut self, event: &Event) -> Vec<String> {
        let mut relay_hints = Vec::new();

        for tag in &event.tags {
            let tag_vec = tag.as_vec();
            if tag_vec.len() >= 2 && tag_vec[0] == "r" {
                relay_hints.push(tag_vec[1].clone());
            }
        }

        let clean_relays = RelayUtils::clean_relays(&relay_hints);

        if !clean_relays.is_empty() {
            // Get existing hints for this pubkey
            let existing = self
                .relay_hints
                .get(&event.pubkey.to_hex())
                .cloned()
                .unwrap_or_default();

            // Create a set to keep track of unique relays
            let mut unique_relays = std::collections::HashSet::new();

            // Add existing relays
            for relay in existing {
                unique_relays.insert(relay);
            }

            // Add new relays
            for relay in &relay_hints {
                unique_relays.insert(relay.clone());
            }

            // Convert back to vec and update
            let updated_relays: Vec<String> = unique_relays.into_iter().collect();
            self.relay_hints
                .insert(event.pubkey.to_hex(), updated_relays);
        }

        clean_relays
    }

    pub fn find_relay_candidates(&self, kind: u64, pubkey: &str, write: &bool) -> Vec<String> {
        let mut relays_found = Vec::new();

        // Check if there are any relay hints for this pubkey
        if let Some(hints) = self.relay_hints.get(pubkey) {
            if !hints.is_empty() {
                relays_found.extend_from_slice(hints);
            }
        }

        match kind {
            10002 | 0 | 10019 => {
                if !self.indexer_relays.is_empty() {
                    let mut counter = self.indexer_relay_counter.write().unwrap();
                    *counter = (*counter + 1) % self.indexer_relays.len();
                    relays_found.push(self.indexer_relays[*counter].clone());
                }
            }
            _ => {
                if *write == true {
                    if let Some(write_relays) = self.get_write_relays(pubkey) {
                        relays_found.extend(write_relays);
                    }
                } else {
                    if let Some(read_relays) = self.get_read_relays(pubkey) {
                        relays_found.extend(read_relays);
                    }
                }
            }
        }

        relays_found = RelayUtils::clean_relays(&relays_found);

        // Ensure we have at least 3 relays
        if relays_found.len() < 3 {
            match kind {
                10002 | 0 | 10019 => {
                    if !self.indexer_relays.is_empty() {
                        let mut counter = self.indexer_relay_counter.write().unwrap();
                        *counter = (*counter + 1) % self.indexer_relays.len();
                        relays_found.push(self.indexer_relays[*counter].clone());
                    }
                }
                _ => {
                    // Add a random relay from defaults
                    if !self.default_relays.is_empty() {
                        let mut counter = self.default_relay_counter.write().unwrap();
                        *counter = (*counter + 1) % self.default_relays.len();
                        let random_relay = &self.default_relays[*counter];
                        if !relays_found.contains(random_relay) {
                            relays_found.push(random_relay.clone());
                        }
                    }
                }
            }
        }

        relays_found
    }

    /// Get database statistics
    pub fn get_stats(&self) -> DatabaseStats {
        let indexes = self
            .indexes
            .read()
            .unwrap_or_else(|_| panic!("Lock poisoned"));
        indexes.get_stats()
    }

    /// Reset and refill persistent storage with recent events
    pub async fn reset_and_refill_storage(&self) -> Result<(), DatabaseError> {
        let events: Vec<ProcessedNostrEvent> = {
            let indexes = self.indexes.read().map_err(|_| DatabaseError::LockError)?;
            indexes.events_by_id.values().cloned().collect()
        };

        // Clear and save events using the storage trait
        self.storage.clear_storage().await?;
        self.storage.save_events(events).await?;

        Ok(())
    }

    /// Save events to persistent storage
    pub async fn save_events_to_storage(
        &self,
        events: Vec<ParsedEvent>,
    ) -> Result<(), DatabaseError> {
        if events.is_empty() {
            return Ok(());
        }

        info!("Saving {} events to persistent storage", events.len());

        let processed_events: Vec<ProcessedNostrEvent> = events
            .into_iter()
            .map(ProcessedNostrEvent::from_parsed_event)
            .collect();

        self.storage.save_events(processed_events).await?;
        Ok(())
    }

    fn should_cache_event(&self, event: &ParsedEvent) -> bool {
        // Match Go implementation - only cache specific kinds
        let kind = event.event.kind.as_u64() as i32;
        match kind {
            0 => true,     // Metadata events
            3 => true,     // Contact lists
            4 => true,     // Direct messages
            10002 => true, // Relay list metadata
            10019 => true, // nuts.cash user settings
            17375 => true, // nuts.cash encrypted wallet event
            _ => false,
        }
    }

    /// Start the periodic save task that flushes the to_save buffer every 5 seconds
    fn start_periodic_save_task(&self) {
        let to_save = self.to_save.clone();
        let storage = self.storage.clone();

        spawn_local(async move {
            loop {
                // Wait for 5 seconds
                TimeoutFuture::new(5000).await;

                // Get events to save
                let events_to_save = {
                    let mut buffer = match to_save.write() {
                        Ok(buffer) => buffer,
                        Err(e) => {
                            error!("Failed to acquire write lock on to_save buffer: {:?}", e);
                            continue;
                        }
                    };

                    if buffer.is_empty() {
                        continue;
                    }

                    let events = buffer.drain(..).collect::<Vec<_>>();
                    events
                };

                // Save to storage
                if !events_to_save.is_empty() {
                    debug!(
                        "Flushing {} events from to_save buffer to storage",
                        events_to_save.len()
                    );

                    let processed_events: Vec<ProcessedNostrEvent> = events_to_save
                        .into_iter()
                        .map(ProcessedNostrEvent::from_parsed_event)
                        .collect();

                    if let Err(e) = storage.save_events(processed_events).await {
                        error!("Failed to save events from buffer to storage: {:?}", e);
                    }
                }
            }
        });
    }

    /// Add an event to the to_save buffer
    pub fn add_event_to_save_buffer(&self, event: ParsedEvent) -> Result<(), DatabaseError> {
        let mut buffer = self.to_save.write().map_err(|_| DatabaseError::LockError)?;
        buffer.push(event);
        Ok(())
    }

    /// Flush the to_save buffer immediately
    pub async fn flush_save_buffer(&self) -> Result<(), DatabaseError> {
        let events_to_save = {
            let mut buffer = self.to_save.write().map_err(|_| DatabaseError::LockError)?;
            if buffer.is_empty() {
                return Ok(());
            }
            buffer.drain(..).collect::<Vec<_>>()
        };

        self.save_events_to_storage(events_to_save).await
    }
}

impl Default for NostrDB {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for NostrDB {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NostrDB")
            .field("config", &self.config)
            .field("is_initialized", &"...")
            .finish()
    }
}

#[async_trait(?Send)]
impl EventDatabase for NostrDB {
    async fn query_events_for_requests(
        &self,
        requests: Vec<Request>,
        cache_only: bool,
    ) -> Result<(Vec<Request>, Vec<ParsedEvent>)> {
        if requests.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let mut all_events = Vec::new();
        let mut remaining_requests = Vec::new();

        for request in requests {
            match request.to_filter() {
                Ok(nostr_filter) => {
                    let filter = QueryFilter::from_nostr_filter(&nostr_filter);
                    let result = self
                        .query_events_internal(filter)
                        .map_err(|e| anyhow::anyhow!("Database query error: {}", e))?;

                    all_events.extend(result.events);

                    // Determine if request should be forwarded to network
                    if !cache_only {
                        if !request.cache_first {
                            remaining_requests.push(request);
                        } else if result.total_found == 0 {
                            remaining_requests.push(request);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to convert request to filter: {}", e);
                    if !cache_only {
                        remaining_requests.push(request);
                    }
                }
            }
        }

        // Sort all events by creation time (newest first)
        all_events.sort_by(|a, b| b.event.created_at.cmp(&a.event.created_at));

        Ok((remaining_requests, all_events))
    }

    async fn query_events(&self, filter: Filter) -> Result<Vec<ParsedEvent>> {
        let query_filter = QueryFilter::from_nostr_filter(&filter);
        debug!(
                "Query filter: kinds={:?}, authors={:?}, ids={:?}, e_tags={:?}, p_tags={:?}, since={:?}, until={:?}, limit={:?}",
                query_filter.kinds.as_ref().map(|k| k.len()),
                query_filter.authors.as_ref().map(|a| a.len()),
                query_filter.ids.as_ref().map(|i| i.len()),
                query_filter.e_tags.as_ref().map(|e| e.len()),
                query_filter.p_tags.as_ref().map(|p| p.len()),
                query_filter.since,
                query_filter.until,
                query_filter.limit
            );
        let result = self
            .query_events_internal(query_filter)
            .map_err(|e| anyhow::anyhow!("Database query error: {}", e))?;
        Ok(result.events)
    }

    async fn add_event(&self, event: ParsedEvent) -> Result<()> {
        if self.should_cache_event(&event) {
            self.add_event_to_save_buffer(event.clone())
                .map_err(|e| warn!("Failed to add event to save buffer: {:?}", e))
                .ok();
        }
        if event.event.id.to_hex().is_empty() {
            return Err(anyhow::anyhow!("Event ID cannot be empty"));
        }

        let processed_event = ProcessedNostrEvent::from_parsed_event(event);
        let event_id = processed_event.id();

        // Add to indexes
        {
            let mut indexes = self.indexes.write().map_err(|_| DatabaseError::LockError)?;
            self.index_event(&mut indexes, processed_event);
        }

        if self.config.debug_logging {
            debug!("Added event {} to database", event_id);
        }

        Ok(())
    }

    async fn save_events_to_persistent_storage(&self, events: Vec<ParsedEvent>) -> Result<()> {
        self.save_events_to_storage(events)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to save events to storage: {}", e))
    }
}

/// Global database instance
static mut GLOBAL_DB: Option<Arc<NostrDB>> = None;
static INIT: std::sync::Once = std::sync::Once::new();

/// Initialize the global database instance
pub async fn init_nostr_db() -> Arc<NostrDB> {
    unsafe {
        INIT.call_once(|| {
            GLOBAL_DB = Some(Arc::new(NostrDB::new()));
        });

        let db = GLOBAL_DB.as_ref().unwrap().clone();

        // Initialize the database if not already done
        if !db.is_initialized() {
            if let Err(e) = db.initialize().await {
                error!("Failed to initialize NostrDB: {}", e);
            }
        }

        db
    }
}

/// Get the global database instance
pub fn get_global_db() -> Option<Arc<NostrDB>> {
    unsafe { GLOBAL_DB.clone() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{Event, EventBuilder, Keys, Kind, Tag};

    async fn create_test_db() -> NostrDB {
        NostrDB::new()
    }

    fn create_test_event(keys: &Keys, kind: Kind, content: &str, tags: Vec<Tag>) -> Event {
        EventBuilder::new(kind, content, tags)
            .to_event(keys)
            .unwrap()
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_add_and_get_event() {
        let db = create_test_db().await;
        let keys = Keys::generate();

        let event = create_test_event(&keys, Kind::TextNote, "Hello world", vec![]);
        let parsed_event = ParsedEvent::new(event.clone());

        db.add_event(parsed_event.clone()).await.unwrap();

        let retrieved = db.get_event(&event.id);
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().event.id, event.id);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_has_event() {
        let db = create_test_db().await;
        let keys = Keys::generate();

        let event = create_test_event(&keys, Kind::TextNote, "Hello world", vec![]);
        let parsed_event = ParsedEvent::new(event.clone());

        assert!(!db.has_event(&event.id));

        db.add_event(parsed_event).await.unwrap();

        assert!(db.has_event(&event.id));
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_query_by_kind() {
        let db = create_test_db().await;
        let keys = Keys::generate();

        // Add different kinds of events
        let note = create_test_event(&keys, Kind::TextNote, "Note", vec![]);
        let reaction = create_test_event(&keys, Kind::Reaction, "+", vec![]);

        db.add_event(ParsedEvent::new(note.clone())).await.unwrap();
        db.add_event(ParsedEvent::new(reaction.clone()))
            .await
            .unwrap();

        // Query for text notes only
        let filter = Filter::new().kind(Kind::TextNote);
        let results = db.query_events(filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].event.kind, Kind::TextNote);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_query_by_author() {
        let db = create_test_db().await;
        let keys1 = Keys::generate();
        let keys2 = Keys::generate();

        let event1 = create_test_event(&keys1, Kind::TextNote, "From user 1", vec![]);
        let event2 = create_test_event(&keys2, Kind::TextNote, "From user 2", vec![]);

        db.add_event(ParsedEvent::new(event1.clone()))
            .await
            .unwrap();
        db.add_event(ParsedEvent::new(event2.clone()))
            .await
            .unwrap();

        // Query for events from keys1 only
        let filter = Filter::new().author(keys1.public_key());
        let results = db.query_events(filter).await.unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].event.pubkey, keys1.public_key());
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_profile_handling() {
        let db = create_test_db().await;
        let keys = Keys::generate();

        let profile_content = r#"{"name":"Test User","about":"A test user"}"#;
        let profile = create_test_event(&keys, Kind::Metadata, profile_content, vec![]);

        db.add_event(ParsedEvent::new(profile.clone()))
            .await
            .unwrap();

        let retrieved_profile = db.get_profile(&keys.public_key());
        assert!(retrieved_profile.is_some());
        assert_eq!(retrieved_profile.unwrap().event.content, profile_content);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_stats() {
        let db = create_test_db().await;
        let keys = Keys::generate();

        // Add multiple events
        for i in 0..5 {
            let event = create_test_event(&keys, Kind::TextNote, &format!("Note {}", i), vec![]);
            db.add_event(ParsedEvent::new(event)).await.unwrap();
        }

        let stats = db.get_stats();
        assert_eq!(stats.total_events, 5);
        assert_eq!(stats.events_by_kind.get(&Kind::TextNote.as_u64()), Some(&5));
    }
}
