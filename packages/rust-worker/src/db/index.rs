use crate::db::ring_buffer::RingBufferStorage;
use crate::db::sharded_storage::ShardedRingBufferStorage;
use crate::db::types::{
    intersect_event_sets, DatabaseConfig, DatabaseError, DatabaseIndexes, EventStorage,
    QueryFilter, QueryResult,
};
use crate::generated::nostr::fb;
use crate::network::interfaces::EventDatabase;
use crate::parsed_event::ParsedEvent;
use crate::types::network::Request;
use crate::utils::relay::RelayUtils;
use crate::{METADATA, RELAY_LIST};
use rustc_hash::{FxHashMap, FxHashSet};

type Result<T> = std::result::Result<T, DatabaseError>;

use crate::types::nostr::{Event, EventId, Filter, PublicKey};
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info, warn};

/// Main NostrDB implementation with RefCell indexes for single-threaded async access
pub struct NostrDB<S = ShardedRingBufferStorage> {
    /// Event indexes
    indexes: DatabaseIndexes,
    /// Persistent storage backend
    storage: S,
    /// Initialization state
    is_initialized: Arc<RwLock<bool>>,
    /// Default relays for nostr operations
    pub default_relays: Vec<String>,
    /// Indexer relays for nostr operations
    pub indexer_relays: Vec<String>,
    /// Relay hints for pubkeys
    pub relay_hints: Arc<RwLock<FxHashMap<String, Vec<String>>>>,
    /// Counter for round-robin indexer relay selection
    indexer_relay_counter: Arc<RwLock<usize>>,
    /// Counter for round-robin default relay selection
    default_relay_counter: Arc<RwLock<usize>>,
}

impl NostrDB<ShardedRingBufferStorage> {
    pub fn new_with_ringbuffer(
        db_name: String,
        buffer_key: String,
        max_buffer_size: usize,
        default_relays: Vec<String>,
        indexer_relays: Vec<String>,
    ) -> Self {
        let storage = ShardedRingBufferStorage::new_default(
            &db_name,
            max_buffer_size, // default kinds ring size
            2 * 1024 * 1024, // kind 0 ring size
            2 * 1024 * 1024, // kind 4 ring size
            1 * 1024 * 1024, // kind 7375 ring size
            DatabaseConfig::default(),
        );
        // let storage = RingBufferStorage::new(db_name, buffer_key, max_buffer_size, config.clone());
        Self {
            indexes: DatabaseIndexes::new(),
            storage,
            is_initialized: Arc::new(RwLock::new(false)),
            default_relays,
            indexer_relays,
            relay_hints: Arc::new(RwLock::new(FxHashMap::default())),
            indexer_relay_counter: Arc::new(RwLock::new(0)),
            default_relay_counter: Arc::new(RwLock::new(0)),
        }
    }
}

impl<S: EventStorage> NostrDB<S> {
    /// Initialize the database by loading events from persistent storage
    pub async fn initialize(&self) -> Result<()> {
        info!("Initializing NostrDB...");

        let mut is_init = self
            .is_initialized
            .write()
            .map_err(|_| DatabaseError::LockError)?;
        if *is_init {
            return Ok(());
        }

        // Clear existing indexes
        self.indexes.clear();

        self.storage.initialize_storage().await?;

        // Load events from storage
        let events = self.storage.load_events()?;

        if !events.is_empty() {
            info!("Loading {} events from persistent storage", events.len());
            self.build_indexes_from_events(events)?;
        }

        *is_init = true;
        let event_count = self.indexes.events_by_id.borrow().len();
        info!(
            "NostrDB initialization complete with {} events in cache",
            event_count
        );

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
    fn build_indexes_from_events(&self, events_offset: Vec<u64>) -> Result<()> {
        // Pre-allocate maps based on event count for better performance
        let event_count = events_offset.len();
        let mut pubkey_frequency = FxHashMap::default();
        let mut kind_frequency = FxHashMap::default();

        // Convert all events to flatbuffer representation
        let mut fb_events = Vec::with_capacity(event_count);

        let offsets = events_offset.clone();
        let mut raw_events: Vec<Vec<u8>> = Vec::new();

        // Phase 1: fetch all bytes
        for event_offset in offsets {
            if let Ok(Some(bytes)) = self.storage.get_event(event_offset) {
                raw_events.push(bytes);
            }
        }

        // Phase 2: parse from stable backing storage
        for bytes in &raw_events {
            if let Some(parsed_event_view) = Self::extract_parsed_event(bytes) {
                fb_events.push(parsed_event_view);
            }
        }

        // First pass: count frequencies for optimal map sizing
        for event in &fb_events {
            *pubkey_frequency.entry(event.pubkey()).or_insert(0) += 1;
            *kind_frequency.entry(event.kind()).or_insert(0) += 1;
        }

        // Pre-allocate maps for high-frequency items
        for (pubkey, count) in pubkey_frequency {
            if count > 5 {
                self.indexes.events_by_pubkey.borrow_mut().insert(
                    pubkey.to_string(),
                    FxHashSet::with_capacity_and_hasher(count, Default::default()),
                );
            }
        }

        for (kind, count) in kind_frequency {
            self.indexes.events_by_kind.borrow_mut().insert(
                kind,
                FxHashSet::with_capacity_and_hasher(count, Default::default()),
            );
        }

        // Second pass: build all indexes
        for (i, event) in fb_events.iter().enumerate() {
            self.index_event(*event, events_offset[i]);
        }

        Ok(())
    }

    /// Extract ParsedEvent from WorkerMessage
    fn extract_parsed_event(worker_message: &Vec<u8>) -> Option<fb::ParsedEvent<'_>> {
        if let Ok(message) = flatbuffers::root::<fb::WorkerMessage>(&worker_message) {
            match message.content_type() {
                fb::Message::ParsedEvent => message.content_as_parsed_event(),
                _ => None,
            }
        } else {
            None
        }
    }

    /// Add an event to all relevant indexes
    fn index_event(&self, event: fb::ParsedEvent<'_>, offset: u64) {
        let event_id = event.id();
        // Primary index
        self.indexes
            .events_by_id
            .borrow_mut()
            .insert(event_id.to_string(), offset);

        // Kind index
        self.indexes
            .events_by_kind
            .borrow_mut()
            .entry(event.kind())
            .or_insert_with(FxHashSet::default)
            .insert(event_id.to_string());

        // Pubkey index
        self.indexes
            .events_by_pubkey
            .borrow_mut()
            .entry(event.pubkey().to_string())
            .or_insert_with(FxHashSet::default)
            .insert(event_id.to_string());

        let tags = event.tags();

        for i in 0..tags.len() {
            let tag = tags.get(i);
            if let Some(tag_vec) = tag.items() {
                if tag_vec.len() >= 2 {
                    let tag_kind = tag_vec.get(0);
                    let tag_value = tag_vec.get(1);

                    match tag_kind {
                        "e" => {
                            self.indexes
                                .events_by_e_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_id.to_string());
                        }
                        "p" => {
                            self.indexes
                                .events_by_p_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_id.to_string());
                        }
                        "a" => {
                            self.indexes
                                .events_by_a_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_id.to_string());
                        }
                        "d" => {
                            self.indexes
                                .events_by_d_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_id.to_string());
                        }
                        _ => {}
                    }
                }
            }
        }

        // // Special handling for profiles (kind 0)
        // if event.kind() == 0 {
        //     self.indexes
        //         .profiles_by_pubkey
        //         .borrow_mut()
        //         .insert(event.pubkey(), event.clone());
        // }

        // // Special handling for relay lists (kind 10002)
        // if event.kind() == 10002 {
        //     self.indexes
        //         .relays_by_pubkey
        //         .borrow_mut()
        //         .insert(event.pubkey(), event);
        // }
    }

    /// Query events using the internal filter format
    pub fn query_events_internal(&self, filter: QueryFilter) -> Result<QueryResult> {
        let start_time = js_sys::Date::now();

        // Start with candidate sets from indexed fields
        let mut candidate_sets = Vec::new();
        let mut use_full_scan = true;

        // Filter by IDs (most specific)
        if let Some(ids) = &filter.ids {
            let mut id_set = FxHashSet::default();
            for id in ids {
                let id_hex = id.to_hex();
                if self.indexes.events_by_id.borrow().contains_key(&id_hex) {
                    id_set.insert(id_hex);
                }
            }
            candidate_sets.push(id_set);
            use_full_scan = false;
        }

        // Filter by kinds
        if let Some(kinds) = &filter.kinds {
            let mut kind_events = FxHashSet::default();
            for kind in kinds {
                if let Some(event_ids) = self.indexes.events_by_kind.borrow().get(kind) {
                    kind_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(kind_events);
            use_full_scan = false;
        }

        // Filter by authors
        if let Some(authors) = &filter.authors {
            let mut author_events = FxHashSet::default();
            for author in authors {
                let author_hex = author.to_hex();
                if let Some(event_ids) = self.indexes.events_by_pubkey.borrow().get(&author_hex) {
                    author_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(author_events);
            use_full_scan = false;
        }

        // Filter by e_tags
        if let Some(e_tags) = &filter.e_tags {
            let mut tag_events = FxHashSet::default();
            for tag in e_tags {
                if let Some(event_ids) = self.indexes.events_by_e_tag.borrow().get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Filter by p_tags
        if let Some(p_tags) = &filter.p_tags {
            let mut tag_events = FxHashSet::default();
            for tag in p_tags {
                if let Some(event_ids) = self.indexes.events_by_p_tag.borrow().get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Filter by a_tags
        if let Some(a_tags) = &filter.a_tags {
            let mut tag_events = FxHashSet::default();
            for tag in a_tags {
                if let Some(event_ids) = self.indexes.events_by_a_tag.borrow().get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Filter by d_tags
        if let Some(d_tags) = &filter.d_tags {
            let mut tag_events = FxHashSet::default();
            for tag in d_tags {
                if let Some(event_ids) = self.indexes.events_by_d_tag.borrow().get(tag) {
                    tag_events.extend(event_ids.iter().cloned());
                }
            }
            candidate_sets.push(tag_events);
            use_full_scan = false;
        }

        // Get final candidate set
        let candidate_ids = if use_full_scan {
            // No indexed filters, scan all events
            self.indexes.events_by_id.borrow().keys().cloned().collect()
        } else if candidate_sets.is_empty() {
            FxHashSet::default()
        } else {
            // Intersect all candidate sets
            let candidate_refs: Vec<&FxHashSet<String>> = candidate_sets.iter().collect();
            intersect_event_sets(candidate_refs)
        };

        // Apply non-indexed filters and collect results
        let mut results = Vec::new();
        let search_lower = filter.search.as_ref().map(|s| s.to_lowercase());

        for event_id in candidate_ids {
            // Clone the event to avoid holding the borrow
            if let Some(offset) = self.indexes.events_by_id.borrow().get(&event_id).cloned() {
                if let Ok(Some(b)) = self.storage.get_event(offset) {
                    if let Some(event) = Self::extract_parsed_event(&b) {
                        // Time range filters
                        if let Some(since) = filter.since {
                            if event.created_at() < since as u32 {
                                continue;
                            }
                        }

                        if let Some(until) = filter.until {
                            if event.created_at() > until as u32 {
                                continue;
                            }
                        }

                        // Search filter
                        // if let Some(search) = &search_lower {
                        //     if !event.content().to_lowercase().contains(search) {
                        //         continue;
                        //     }
                        // }

                        // Store the underlying bytes (event borrowing b prevents moving b)
                        results.push(b);
                    }
                }
            }
        }

        let total_found = results.len();

        // Sort by created_at (newest first)
        results.sort_by(|a, b| {
            let a_event = Self::extract_parsed_event(a).unwrap();
            let b_event = Self::extract_parsed_event(b).unwrap();
            b_event.created_at().cmp(&a_event.created_at())
        });

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

        let query_time = js_sys::Date::now() - start_time;

        Ok(QueryResult {
            events: results,
            total_found,
            has_more,
            query_time_ms: query_time as u64,
        })
    }

    /// Get a single event by ID
    pub fn get_event(&self, id: &EventId) -> Option<Vec<u8>> {
        let offset = self
            .indexes
            .events_by_id
            .borrow()
            .get(&id.to_hex())
            .cloned()?;

        self.storage.get_event(offset).ok().flatten()
    }

    /// Check if an event exists
    pub fn has_event(&self, id: &EventId) -> bool {
        self.indexes
            .events_by_id
            .borrow()
            .contains_key(&id.to_hex())
    }

    /// Get a profile for a given pubkey
    pub fn get_profile(&self, pubkey: &PublicKey) -> Option<Vec<u8>> {
        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![METADATA]);
        filter.authors = Some(vec![pubkey.clone()]);
        filter.limit = Some(1);
        let events = self.query_events_internal(filter);
        match events {
            Ok(result) => {
                if !result.events.is_empty() {
                    Some(result.events[0].clone())
                } else {
                    None
                }
            }
            Err(_) => None,
        }
    }

    pub fn get_read_relays(&self, pubkey: &str) -> Option<Vec<String>> {
        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![RELAY_LIST]);
        filter.authors = Some(vec![PublicKey::from_hex(pubkey).ok()?]);
        filter.limit = Some(1);
        let events = self.query_events_internal(filter);
        match events {
            Ok(result) => {
                if !result.events.is_empty() {
                    // Parse the flatbuffer event to get relay data
                    if let Some(event) = Self::extract_parsed_event(&result.events[0]) {
                        if let Some(kind10002) = event.parsed_as_kind_10002_parsed() {
                            return Some(
                                kind10002
                                    .relays()
                                    .iter()
                                    .filter(|relay| relay.read())
                                    .map(|relay| relay.url().to_string())
                                    .collect::<Vec<_>>(),
                            );
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }

    pub fn get_write_relays(&self, pubkey: &str) -> Option<Vec<String>> {
        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![RELAY_LIST]);
        filter.authors = Some(vec![PublicKey::from_hex(pubkey).ok()?]);
        filter.limit = Some(1);
        let events = self.query_events_internal(filter);
        match events {
            Ok(result) => {
                if !result.events.is_empty() {
                    // Parse the flatbuffer event to get relay data
                    if let Some(event) = Self::extract_parsed_event(&result.events[0]) {
                        if let Some(kind10002) = event.parsed_as_kind_10002_parsed() {
                            return Some(
                                kind10002
                                    .relays()
                                    .iter()
                                    .filter(|relay| relay.write())
                                    .map(|relay| relay.url().to_string())
                                    .collect::<Vec<_>>(),
                            );
                        } else {
                            return None;
                        }
                    } else {
                        return None;
                    }
                } else {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }

    pub fn get_relay_hint(&self, event: &Event) -> Vec<String> {
        let mut relay_hints = Vec::new();

        for tag in &event.tags {
            if tag.len() >= 2 && tag[0] == "r" {
                relay_hints.push(tag[1].clone());
            }
        }

        let clean_relays = RelayUtils::clean_relays(&relay_hints);

        if !clean_relays.is_empty() {
            // Get existing hints for this pubkey
            let mut hints = self.relay_hints.write().unwrap();
            let existing = hints
                .get(&event.pubkey.to_hex())
                .cloned()
                .unwrap_or_default();

            // Create a set to keep track of unique relays
            let mut unique_relays = FxHashSet::default();

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
            hints.insert(event.pubkey.to_hex(), updated_relays);
        }

        clean_relays
    }

    pub fn find_relay_candidates(&self, kind: u16, pubkey: &str, write: &bool) -> Vec<String> {
        let mut relays_found = Vec::new();

        // Check if there are any relay hints for this pubkey
        if let Some(hints) = self.relay_hints.read().unwrap().get(pubkey).cloned() {
            if !hints.is_empty() {
                relays_found.extend_from_slice(&hints);
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

    /// Process a single event (the actual indexing logic)
    async fn process_single_event(&self, event: &ParsedEvent) -> Result<()> {
        if event.event.id.to_hex().is_empty() {
            return Err(DatabaseError::StorageError(
                "Event ID cannot be empty".to_string(),
            ));
        }
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let fb_parsed_event = match event.build_flatbuffer(&mut fbb) {
            Ok(parsed_event) => parsed_event,
            Err(e) => {
                warn!("Failed to build flatbuffer for event: {:?}", e);
                return Err(DatabaseError::StorageError(format!(
                    "Failed to build flatbuffer for event: {:?}",
                    e
                )));
            }
        };

        let union_value = fb_parsed_event.as_union_value();

        let message_args = fb::WorkerMessageArgs {
            type_: fb::MessageType::ParsedNostrEvent,
            content_type: fb::Message::ParsedEvent,
            content: Some(union_value),
        };

        let root = fb::WorkerMessage::create(&mut fbb, &message_args);

        // Finish the flatbuffer to get the bytes
        fbb.finish(root, None);
        let finished_data = fbb.finished_data();

        // Parse the flatbuffer to get the event for indexing
        if let Ok(worker_message) = flatbuffers::root::<fb::WorkerMessage>(&finished_data) {
            // Add to indexes
            if let Some(parsed_event) = worker_message.content_as_parsed_event() {
                let offset: u64;
                if let Some(sharded) = self
                    .storage
                    .as_any()
                    .downcast_ref::<ShardedRingBufferStorage>()
                {
                    offset = sharded
                        .add_event_for_kind(event.event.kind as u32, &finished_data)
                        .await?;
                } else {
                    offset = self.storage.add_event_data(&finished_data).await?;
                }
                // let offset = self.storage.add_event_data(&finished_data).await?;
                self.index_event(parsed_event, offset);
            } else {
                warn!("Failed to get parsed event from worker message");
                return Err(DatabaseError::StorageError(
                    "Failed to get parsed event from worker message".to_string(),
                ));
            }
        } else {
            warn!("Failed to parse flatbuffer data for indexing");
            return Err(DatabaseError::StorageError(
                "Failed to parse flatbuffer data".to_string(),
            ));
        }

        Ok(())
    }
}

impl<S: EventStorage> EventDatabase for NostrDB<S> {
    async fn query_events_for_requests(
        &self,
        requests: Vec<Request>,
        cache_only: bool,
    ) -> std::result::Result<(Vec<Request>, Vec<Vec<u8>>), crate::NostrError> {
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
                        .map_err(|e| crate::NostrError::from(e))?;

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
        all_events.sort_by(|a, b| {
            let a_event = Self::extract_parsed_event(a).unwrap();
            let b_event = Self::extract_parsed_event(b).unwrap();
            b_event.created_at().cmp(&a_event.created_at())
        });

        Ok((remaining_requests, all_events))
    }

    async fn query_events(
        &self,
        filter: Filter,
    ) -> std::result::Result<Vec<Vec<u8>>, crate::NostrError> {
        let query_filter = QueryFilter::from_nostr_filter(&filter);
        let result = self
            .query_events_internal(query_filter)
            .map_err(|e| crate::NostrError::from(e))?;
        Ok(result.events)
    }

    async fn add_event(&self, event: &ParsedEvent) -> std::result::Result<(), crate::NostrError> {
        // Validate event ID early
        if event.event.id.to_hex().is_empty() {
            return Err(crate::NostrError::from(DatabaseError::StorageError(
                "Event ID cannot be empty".to_string(),
            )));
        }

        self.process_single_event(event).await.ok();

        Ok(())
    }
}
