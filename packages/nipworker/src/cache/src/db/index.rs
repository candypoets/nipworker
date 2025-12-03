use crate::db::sharded_storage::ShardedRingBufferStorage;
use crate::db::types::{
    intersect_event_sets, DatabaseConfig, DatabaseError, DatabaseIndexes, EventStorage,
    QueryFilter, QueryResult,
};
use crate::generated::nostr::fb;
use crate::sab_ring::SabRing;
use js_sys::SharedArrayBuffer;
use rustc_hash::{FxHashMap, FxHashSet};

type Result<T> = std::result::Result<T, DatabaseError>;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// Main NostrDB implementation with RefCell indexes for single-threaded async access
pub struct NostrDB<S = ShardedRingBufferStorage> {
    /// Event indexes
    indexes: DatabaseIndexes,
    /// Persistent storage backend
    storage: S,
    ingest_ring: Option<Rc<RefCell<SabRing>>>,
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
        max_buffer_size: usize,
        ingest_sab: SharedArrayBuffer,
        default_relays: Vec<String>,
        indexer_relays: Vec<String>,
    ) -> Self {
        let storage = ShardedRingBufferStorage::new_default(
            &db_name,
            max_buffer_size, // default kinds ring size
            2 * 1024 * 1024, // kind 0 ring size
            2 * 1024 * 1024, // kind 4 ring size
            1 * 1024 * 1024, // kind 7375 ring size
            1 * 1024 * 1024,
            DatabaseConfig::default(),
        );

        let ring = SabRing::new(ingest_sab).expect("invalid ingest SAB");

        // let storage = RingBufferStorage::new(db_name, buffer_key, max_buffer_size, config.clone());
        Self {
            indexes: DatabaseIndexes::new(),
            storage,
            ingest_ring: Some(Rc::new(RefCell::new(ring))),
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

    // Drain the ingest SAB and index any new events
    pub async fn sync_from_ingest_ring(&self) -> Result<usize> {
        let Some(ring_rc) = &self.ingest_ring else {
            return Ok(0);
        };
        let mut ring = ring_rc.borrow_mut();
        let mut count = 0usize;
        loop {
            let Some(payload) = ring.read_next() else {
                break;
            };
            // Each payload is expected to be a WorkerMessage (serialized)
            if let Err(e) = self.add_worker_message_bytes(&payload).await {
                warn!("Failed to ingest WorkerMessage: {}", e);
            } else {
                count += 1;
            }
        }
        Ok(count)
    }

    // Feed one WorkerMessage (serialized) coming from the ingest SAB.
    // If it's a ParsedEvent, persist and index it.
    pub async fn add_worker_message_bytes(&self, bytes: &[u8]) -> Result<()> {
        let wm = flatbuffers::root::<fb::WorkerMessage>(bytes)
            .map_err(|e| DatabaseError::StorageError(format!("FB decode error: {:?}", e)))?;

        let Some(parsed) = wm.content_as_parsed_event() else {
            // Ignore non-event messages in the DB store
            return Ok(());
        };

        // Persist the exact bytes into ring storage (shard by kind if available)
        let offset = if let Some(sharded) = self
            .storage
            .as_any()
            .downcast_ref::<ShardedRingBufferStorage>()
        {
            sharded
                .add_event_for_kind(parsed.kind() as u32, bytes)
                .await?
        } else {
            self.storage.add_event_data(bytes).await?
        };

        // Index
        self.index_event(parsed, offset);
        Ok(())
    }

    fn query_filter_from_fb_request(fb_req: &fb::Request<'_>) -> Result<QueryFilter> {
        let mut f = QueryFilter::new();

        // ids (strings)
        if let Some(ids_vec) = fb_req.ids() {
            let ids: Vec<String> = ids_vec.iter().map(|s| s.to_string()).collect();
            if !ids.is_empty() {
                f.ids = Some(ids);
            }
        }

        // authors (strings)
        if let Some(auth_vec) = fb_req.authors() {
            let authors: Vec<String> = auth_vec.iter().map(|s| s.to_string()).collect();
            if !authors.is_empty() {
                f.authors = Some(authors);
            }
        }

        // kinds (u16)
        if let Some(kinds_vec) = fb_req.kinds() {
            let kinds: Vec<u16> = kinds_vec.into_iter().collect();
            if !kinds.is_empty() {
                f.kinds = Some(kinds);
            }
        }

        // tags vector: StringVec where items[0] is key ("e"/"#e" etc), items[1..] are values
        if let Some(tags_vec) = fb_req.tags() {
            let mut e_tags: Vec<String> = Vec::new();
            let mut p_tags: Vec<String> = Vec::new();
            let mut a_tags: Vec<String> = Vec::new();
            let mut d_tags: Vec<String> = Vec::new();

            for i in 0..tags_vec.len() {
                let sv = tags_vec.get(i);
                if let Some(items) = sv.items() {
                    if items.len() >= 2 {
                        let mut key = items.get(0).to_string();
                        if let Some(stripped) = key.strip_prefix('#') {
                            key = stripped.to_string();
                        }
                        let values: Vec<String> =
                            (1..items.len()).map(|j| items.get(j).to_string()).collect();
                        match key.as_str() {
                            "e" => e_tags.extend(values),
                            "p" => p_tags.extend(values),
                            "a" => a_tags.extend(values),
                            "d" => d_tags.extend(values),
                            _ => { /* ignore unknown filter tags */ }
                        }
                    }
                }
            }
            if !e_tags.is_empty() {
                f.e_tags = Some(e_tags);
            }
            if !p_tags.is_empty() {
                f.p_tags = Some(p_tags);
            }
            if !a_tags.is_empty() {
                f.a_tags = Some(a_tags);
            }
            if !d_tags.is_empty() {
                f.d_tags = Some(d_tags);
            }
        }

        // since/until/limit/search
        let since = fb_req.since();
        if since > 0 {
            f.since = Some(since as u32);
        }

        let until = fb_req.until();
        if until > 0 {
            f.until = Some(until as u32);
        }

        let limit = fb_req.limit();
        if limit > 0 {
            f.limit = Some(limit as usize);
        }

        if let Some(s) = fb_req.search() {
            if !s.is_empty() {
                f.search = Some(s.to_string());
            }
        }

        Ok(f)
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

    pub fn query_events_with_filter(&self, filter: QueryFilter) -> Result<QueryResult> {
        let start_time = js_sys::Date::now();
        // Start with candidate sets from indexed fields
        let mut candidate_sets = Vec::new();
        let mut use_full_scan = true;

        // Filter by IDs (most specific)
        if let Some(ids) = &filter.ids {
            let mut id_set = FxHashSet::default();
            for id in ids {
                if self.indexes.events_by_id.borrow().contains_key(id) {
                    id_set.insert(id.clone());
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
                if let Some(event_ids) = self.indexes.events_by_pubkey.borrow().get(author) {
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

    /// Query events using the internal filter format
    pub fn query_events(&self, fb_req: &fb::Request<'_>) -> Result<QueryResult> {
        let filter = Self::query_filter_from_fb_request(fb_req)?;
        self.query_events_with_filter(filter)
    }

    pub async fn query_events_and_requests(
        &self,
        fb_reqs: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<fb::Request<'_>>>>,
    ) -> Result<(Vec<usize>, Vec<Vec<u8>>)> {
        let mut remaining_indices: Vec<usize> = Vec::new();
        let mut all_events: Vec<Vec<u8>> = Vec::new();

        if let Some(vec) = fb_reqs {
            for i in 0..vec.len() {
                let req = vec.get(i);

                // Respect no_cache in request
                if req.no_cache() {
                    continue;
                }

                let result = match self.query_events(&req) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("query_events failed for request {}: {}", i, e);
                        // Treat as remaining (forward to network)
                        remaining_indices.push(i);
                        continue;
                    }
                };

                // accumulate cached events
                all_events.extend(result.events);

                // If cache_first is false -> always forward
                // If cache_first is true -> forward only when result is empty
                if !req.cache_first() || result.total_found == 0 {
                    remaining_indices.push(i);
                }
            }
        }

        // Sort newest-first (same as other paths)
        all_events.sort_by(|a, b| {
            let a_event = Self::extract_parsed_event(a).unwrap();
            let b_event = Self::extract_parsed_event(b).unwrap();
            b_event.created_at().cmp(&a_event.created_at())
        });

        Ok((remaining_indices, all_events))
    }

    /// Get a single event by ID
    pub fn get_event(&self, id: &str) -> Option<Vec<u8>> {
        let offset = self.indexes.events_by_id.borrow().get(id).cloned()?;

        self.storage.get_event(offset).ok().flatten()
    }

    /// Check if an event exists
    pub fn has_event(&self, id: &str) -> bool {
        self.indexes.events_by_id.borrow().contains_key(id)
    }

    /// Get a profile for a given pubkey
    pub fn get_profile(&self, pubkey: &str) -> Option<Vec<u8>> {
        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![0]);
        filter.authors = Some(vec![pubkey.to_string()]);
        filter.limit = Some(1);
        let events = self.query_events_with_filter(filter);
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
        filter.kinds = Some(vec![10002]);
        filter.authors = Some(vec![pubkey.to_string()]);
        filter.limit = Some(1);
        let events = self.query_events_with_filter(filter);
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
        filter.kinds = Some(vec![10002]);
        filter.authors = Some(vec![pubkey.to_string()]);
        filter.limit = Some(1);
        let events = self.query_events_with_filter(filter);
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
}
