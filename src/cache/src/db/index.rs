use crate::db::sharded_storage::ShardedRingBufferStorage;
use crate::db::types::{
    intersect_event_sets, DatabaseConfig, DatabaseError, DatabaseIndexes, EventStorage,
    QueryFilter, QueryResult,
};
use rustc_hash::{FxHashMap, FxHashSet};
use shared::generated::nostr::fb::{self, NostrEvent, ParsedEvent, Request, WorkerMessage};

type Result<T> = std::result::Result<T, DatabaseError>;

use std::future::Future;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

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
}

impl NostrDB<ShardedRingBufferStorage> {
    /// Create a new NostrDB instance
    pub fn new(
        db_name: String,
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
            1 * 1024 * 1024,
            DatabaseConfig::default(),
        );

        Self {
            indexes: DatabaseIndexes::new(),
            storage,
            is_initialized: Arc::new(RwLock::new(false)),
            default_relays,
            indexer_relays,
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

    pub async fn add_worker_message_bytes(&self, bytes: &[u8]) -> Result<()> {
        // Try ParsedEvent first
        if let Ok(parsed) = flatbuffers::root::<ParsedEvent>(bytes) {
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

            self.index_parsed_event(parsed, offset);
            return Ok(());
        }

        // Fallback: NostrEvent
        if let Ok(nostr) = flatbuffers::root::<NostrEvent>(bytes) {
            let offset = if let Some(sharded) = self
                .storage
                .as_any()
                .downcast_ref::<ShardedRingBufferStorage>()
            {
                sharded
                    .add_event_for_kind(nostr.kind() as u32, bytes)
                    .await?
            } else {
                self.storage.add_event_data(bytes).await?
            };

            // Index minimal fields
            self.index_nostr_event(nostr, offset);
            return Ok(());
        }

        Err(DatabaseError::StorageError(
            "FB decode error: expected ParsedEvent or NostrEvent".to_string(),
        ))
    }

    /// Add an event directly from a NostrEvent flatbuffer (for MessageChannel-based architecture)
    pub async fn add_event_from_fb(&self, event: NostrEvent<'_>) -> Result<()> {
        // Serialize the flatbuffer event to bytes for storage
        // Since we can't directly serialize a flatbuffer table, we need to create a new one
        let mut builder = flatbuffers::FlatBufferBuilder::new();

        // Create the NostrEvent in the builder
        let id = builder.create_string(event.id());
        let pubkey = builder.create_string(event.pubkey());
        let sig = builder.create_string(event.sig());
        let content = builder.create_string(event.content());

        // Copy tags
        let tags_vec = event.tags();
        let mut tags_wip: Vec<flatbuffers::WIPOffset<fb::StringVec<'_>>> = Vec::new();
        for i in 0..tags_vec.len() {
            let tag = tags_vec.get(i);
            if let Some(items) = tag.items() {
                let item_strings: Vec<_> = (0..items.len())
                    .map(|j| builder.create_string(items.get(j)))
                    .collect();
                let items_vec = builder.create_vector(&item_strings);
                tags_wip.push(fb::StringVec::create(
                    &mut builder,
                    &fb::StringVecArgs {
                        items: Some(items_vec),
                    },
                ));
            }
        }
        let tags = builder.create_vector(&tags_wip);

        let event_fb = fb::NostrEvent::create(
            &mut builder,
            &fb::NostrEventArgs {
                id: Some(id),
                pubkey: Some(pubkey),
                created_at: event.created_at(),
                kind: event.kind(),
                tags: Some(tags),
                content: Some(content),
                sig: Some(sig),
            },
        );
        builder.finish(event_fb, None);
        let bytes = builder.finished_data().to_vec();

        // Store and index the event
        let offset = if let Some(sharded) = self
            .storage
            .as_any()
            .downcast_ref::<ShardedRingBufferStorage>()
        {
            sharded
                .add_event_for_kind(event.kind() as u32, &bytes)
                .await?
        } else {
            self.storage.add_event_data(&bytes).await?
        };

        // Index the event
        self.index_nostr_event(event, offset);
        Ok(())
    }

    /// Add a ParsedEvent from WorkerMessage bytes - stores bytes as-is!
    /// This preserves decrypted content (e.g., kind4 DMs) without re-decryption.
    pub async fn add_parsed_event_with_bytes(
        &self,
        parsed: ParsedEvent<'_>,
        worker_msg_bytes: &[u8],
    ) -> Result<()> {
        // Store WorkerMessage bytes directly (no rebuilding!)
        let offset = if let Some(sharded) = self
            .storage
            .as_any()
            .downcast_ref::<ShardedRingBufferStorage>()
        {
            sharded
                .add_event_for_kind(parsed.kind() as u32, worker_msg_bytes)
                .await?
        } else {
            self.storage.add_event_data(worker_msg_bytes).await?
        };

        // Index using parsed event fields
        self.index_parsed_event(parsed, offset);
        Ok(())
    }

    /// Add a NostrEvent from WorkerMessage bytes - stores bytes as-is!
    pub async fn add_nostr_event_with_bytes(
        &self,
        event: NostrEvent<'_>,
        worker_msg_bytes: &[u8],
    ) -> Result<()> {
        // Store WorkerMessage bytes directly
        let offset = if let Some(sharded) = self
            .storage
            .as_any()
            .downcast_ref::<ShardedRingBufferStorage>()
        {
            sharded
                .add_event_for_kind(event.kind() as u32, worker_msg_bytes)
                .await?
        } else {
            self.storage.add_event_data(worker_msg_bytes).await?
        };

        // Index using nostr event fields
        self.index_nostr_event(event, offset);
        Ok(())
    }

    #[allow(non_snake_case)]
    fn query_filter_from_fb_request(fb_req: &Request<'_>) -> Result<QueryFilter> {
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
            let mut E_tags: Vec<String> = Vec::new();
            let mut p_tags: Vec<String> = Vec::new();
            let mut P_tags: Vec<String> = Vec::new();
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
                            "E" => E_tags.extend(values),
                            "p" => p_tags.extend(values),
                            "P" => P_tags.extend(values),
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
            if !E_tags.is_empty() {
                f.E_tags = Some(E_tags);
            }
            if !p_tags.is_empty() {
                f.p_tags = Some(p_tags);
            }
            if !P_tags.is_empty() {
                f.P_tags = Some(P_tags);
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
        let offsets = events_offset.clone();
        let mut raw_events: Vec<Vec<u8>> = Vec::new();

        for event_offset in offsets {
            if let Ok(Some(bytes)) = self.storage.get_event(event_offset) {
                raw_events.push(bytes);
            }
        }

        info!("build_indexes_from_events: loaded {} events from storage", raw_events.len());

        // Optional: pre-allocate based on frequencies
        // Note: Events are now stored as WorkerMessage, so we need to unwrap them
        let mut pubkey_frequency = FxHashMap::default();
        let mut kind_frequency = FxHashMap::default();
        let mut worker_message_count = 0;
        let mut legacy_count = 0;
        
        for bytes in &raw_events {
            // Try WorkerMessage first (new format)
            if let Ok(wm) = flatbuffers::root::<WorkerMessage>(bytes) {
                worker_message_count += 1;
                match wm.content_type() {
                    fb::Message::ParsedEvent => {
                        if let Some(p) = wm.content_as_parsed_event() {
                            *pubkey_frequency.entry(p.pubkey().to_string()).or_insert(0) += 1;
                            *kind_frequency.entry(p.kind()).or_insert(0) += 1;
                        }
                    }
                    fb::Message::NostrEvent => {
                        if let Some(n) = wm.content_as_nostr_event() {
                            *pubkey_frequency.entry(n.pubkey().to_string()).or_insert(0) += 1;
                            *kind_frequency.entry(n.kind()).or_insert(0) += 1;
                        }
                    }
                    _ => {}
                }
            } else {
                // Legacy format: direct ParsedEvent or NostrEvent
                legacy_count += 1;
                if let Ok(p) = flatbuffers::root::<ParsedEvent>(bytes) {
                    *pubkey_frequency.entry(p.pubkey().to_string()).or_insert(0) += 1;
                    *kind_frequency.entry(p.kind()).or_insert(0) += 1;
                } else if let Ok(n) = flatbuffers::root::<NostrEvent>(bytes) {
                    *pubkey_frequency.entry(n.pubkey().to_string()).or_insert(0) += 1;
                    *kind_frequency.entry(n.kind()).or_insert(0) += 1;
                }
            }
        }
        
        info!("build_indexes_from_events: {} WorkerMessage format, {} legacy format", 
              worker_message_count, legacy_count);
        
        for (pubkey, count) in pubkey_frequency {
            if count > 5 {
                self.indexes.events_by_pubkey.borrow_mut().insert(
                    pubkey,
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

        // Index everything
        let mut indexed_count = 0;
        for (i, bytes) in raw_events.iter().enumerate() {
            // Try WorkerMessage first (new format)
            if let Ok(wm) = flatbuffers::root::<WorkerMessage>(bytes) {
                match wm.content_type() {
                    fb::Message::ParsedEvent => {
                        if let Some(p) = wm.content_as_parsed_event() {
                            self.index_parsed_event(p, events_offset[i]);
                            indexed_count += 1;
                        }
                    }
                    fb::Message::NostrEvent => {
                        if let Some(n) = wm.content_as_nostr_event() {
                            self.index_nostr_event(n, events_offset[i]);
                            indexed_count += 1;
                        }
                    }
                    _ => {}
                }
            } else {
                // Legacy format
                if let Ok(p) = flatbuffers::root::<ParsedEvent>(bytes) {
                    self.index_parsed_event(p, events_offset[i]);
                    indexed_count += 1;
                } else if let Ok(n) = flatbuffers::root::<NostrEvent>(bytes) {
                    self.index_nostr_event(n, events_offset[i]);
                    indexed_count += 1;
                }
            }
        }

        info!("build_indexes_from_events: indexed {} events", indexed_count);
        Ok(())
    }

    /// Try to extract a ParsedEvent from raw bytes
    /// Handles both WorkerMessage wrapper (new format) and direct event (legacy)
    fn extract_parsed_event(bytes: &Vec<u8>) -> Option<ParsedEvent<'_>> {
        // Try WorkerMessage first (new format)
        if let Ok(wm) = flatbuffers::root::<WorkerMessage>(bytes) {
            if wm.content_type() == fb::Message::ParsedEvent {
                return wm.content_as_parsed_event();
            }
            return None;
        }
        // Legacy format: direct ParsedEvent
        flatbuffers::root::<ParsedEvent>(bytes).ok()
    }

    /// Try to extract a NostrEvent from raw bytes
    /// Handles both WorkerMessage wrapper (new format) and direct event (legacy)
    fn extract_nostr_event(bytes: &Vec<u8>) -> Option<NostrEvent<'_>> {
        // Try WorkerMessage first (new format)
        if let Ok(wm) = flatbuffers::root::<WorkerMessage>(bytes) {
            if wm.content_type() == fb::Message::NostrEvent {
                return wm.content_as_nostr_event();
            }
            return None;
        }
        // Legacy format: direct NostrEvent
        flatbuffers::root::<NostrEvent>(bytes).ok()
    }

    /// Extract created_at regardless of format (ParsedEvent or NostrEvent)
    /// Handles both WorkerMessage wrapper (new format) and direct event (legacy)
    fn extract_created_at(bytes: &Vec<u8>) -> Option<u32> {
        // Try WorkerMessage first (new format)
        if let Ok(wm) = flatbuffers::root::<WorkerMessage>(bytes) {
            match wm.content_type() {
                fb::Message::ParsedEvent => {
                    if let Some(p) = wm.content_as_parsed_event() {
                        return Some(p.created_at());
                    }
                }
                fb::Message::NostrEvent => {
                    if let Some(n) = wm.content_as_nostr_event() {
                        return Some(n.created_at().max(0) as u32);
                    }
                }
                _ => {}
            }
            return None;
        }
        // Legacy format: direct events
        if let Ok(p) = flatbuffers::root::<ParsedEvent>(bytes) {
            return Some(p.created_at());
        }
        if let Ok(n) = flatbuffers::root::<NostrEvent>(bytes) {
            return Some(n.created_at().max(0) as u32);
        }
        None
    }

    /// Add an event to all relevant indexes
    fn index_parsed_event(&self, event: ParsedEvent<'_>, offset: u64) {
        let event_id = event.id();

        // Primary
        self.indexes
            .events_by_id
            .borrow_mut()
            .insert(event_id.to_string(), offset);

        // Kind
        self.indexes
            .events_by_kind
            .borrow_mut()
            .entry(event.kind())
            .or_insert_with(FxHashSet::default)
            .insert(event_id.to_string());

        // Pubkey
        self.indexes
            .events_by_pubkey
            .borrow_mut()
            .entry(event.pubkey().to_string())
            .or_insert_with(FxHashSet::default)
            .insert(event_id.to_string());

        // Tags
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
                        "E" => {
                            // Uppercase E - NIP-22 "E" tag (event id reference)
                            self.indexes
                                .events_by_E_tag
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
                        "P" => {
                            // Uppercase P - NIP-22 "P" tag (pubkey reference)
                            self.indexes
                                .events_by_P_tag
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
    }

    fn index_nostr_event(&self, event: NostrEvent<'_>, offset: u64) {
        let event_id = event.id();

        // Primary
        self.indexes
            .events_by_id
            .borrow_mut()
            .insert(event_id.to_string(), offset);

        // Kind
        self.indexes
            .events_by_kind
            .borrow_mut()
            .entry(event.kind())
            .or_insert_with(FxHashSet::default)
            .insert(event_id.to_string());

        // Pubkey
        self.indexes
            .events_by_pubkey
            .borrow_mut()
            .entry(event.pubkey().to_string())
            .or_insert_with(FxHashSet::default)
            .insert(event_id.to_string());

        // Tags
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
                        "E" => {
                            // Uppercase E - NIP-22 "E" tag (event id reference)
                            self.indexes
                                .events_by_E_tag
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
                        "P" => {
                            // Uppercase P - NIP-22 "P" tag (pubkey reference)
                            self.indexes
                                .events_by_P_tag
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
    }

    #[allow(non_snake_case)]
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

        // Filter by E_tags (uppercase - NIP-22)
        if let Some(E_tags) = &filter.E_tags {
            let mut tag_events = FxHashSet::default();
            for tag in E_tags {
                if let Some(event_ids) = self.indexes.events_by_E_tag.borrow().get(tag) {
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

        // Filter by P_tags (uppercase - NIP-22)
        if let Some(P_tags) = &filter.P_tags {
            let mut tag_events = FxHashSet::default();
            for tag in P_tags {
                if let Some(event_ids) = self.indexes.events_by_P_tag.borrow().get(tag) {
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
                    } else if let Some(n) = Self::extract_nostr_event(&b) {
                        // Time range filters for NostrEvent
                        if let Some(since) = filter.since {
                            if (n.created_at().max(0) as u32) < since as u32 {
                                continue;
                            }
                        }

                        if let Some(until) = filter.until {
                            if (n.created_at().max(0) as u32) > until as u32 {
                                continue;
                            }
                        }

                        // Search filter
                        // if let Some(search) = &search_lower {
                        //     if !n.content().to_lowercase().contains(search) {
                        //         continue;
                        //     }
                        // }

                        // Store the underlying bytes
                        results.push(b);
                    }
                }
            }
        }

        let total_found = results.len();

        // Sort by created_at (newest first)
        results.sort_by(|a, b| {
            let ca = Self::extract_created_at(a).unwrap_or_default();
            let cb = Self::extract_created_at(b).unwrap_or_default();
            cb.cmp(&ca)
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
    pub fn query_events(&self, fb_req: &Request<'_>) -> Result<QueryResult> {
        let filter = Self::query_filter_from_fb_request(fb_req)?;
        info!("query_events: filter kinds={:?}, authors={:?}, limit={:?}", 
              filter.kinds, filter.authors, filter.limit);
        self.query_events_with_filter(filter)
    }

    pub async fn query_events_and_requests(
        &self,
        fb_reqs: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<Request<'_>>>>,
    ) -> Result<(Vec<usize>, Vec<Vec<u8>>)> {
        let mut remaining_indices: Vec<usize> = Vec::new();
        let mut all_events: Vec<Vec<u8>> = Vec::new();

        if let Some(vec) = fb_reqs {
            info!("query_events_and_requests: processing {} requests", vec.len());
            for i in 0..vec.len() {
                let req = vec.get(i);

                // Respect no_cache in request
                if req.no_cache() {
                    info!("Request {} has no_cache=true, forwarding to network", i);
                    remaining_indices.push(i);
                    continue;
                }

                info!("Request {}: querying cache...", i);
                let result = match self.query_events(&req) {
                    Ok(r) => {
                        info!("Request {}: found {} events in cache", i, r.total_found);
                        r
                    }
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
                    info!("Request {}: forwarding to network (cache_first={}, total_found={})", 
                          i, req.cache_first(), result.total_found);
                    remaining_indices.push(i);
                }
            }
        } else {
            info!("query_events_and_requests: no requests provided");
        }

        // Sort newest-first (same as other paths)
        all_events.sort_by(|a, b| {
            let ca = Self::extract_created_at(a).unwrap_or_default();
            let cb = Self::extract_created_at(b).unwrap_or_default();
            cb.cmp(&ca)
        });

        info!("query_events_and_requests: returning {} events, {} remaining indices", 
              all_events.len(), remaining_indices.len());

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

    pub fn get_relays(&self, fb_req: &Request<'_>) -> Vec<String> {
        let mut relay_counts: FxHashMap<String, usize> = FxHashMap::default();

        // Collect all pubkeys we need to check relays for
        let mut pubkeys_to_check = Vec::new();
        let mut authors_set = FxHashSet::default();

        // Add authors from the request (these will need write relays)
        if let Some(authors) = fb_req.authors() {
            for author in authors {
                authors_set.insert(author.to_string());
                pubkeys_to_check.push(author.to_string());
            }
        }

        // Also check for pubkeys mentioned in tags (p tags) - these will need read relays
        // This helps find relays for events we're querying about
        if let Some(tags) = fb_req.tags() {
            for tag in tags {
                if let Some(items) = tag.items() {
                    if items.len() > 1 && items.get(0) == "p" {
                        let pubkey = items.get(1).to_string();
                        if !authors_set.contains(&pubkey) {
                            pubkeys_to_check.push(pubkey);
                        }
                    }
                }
            }
        }

        // If no pubkeys found, check if we need fallback relays
        if pubkeys_to_check.is_empty() {
            // Check if the request is for indexer kinds (0, 3, 10002)
            if let Some(kinds) = fb_req.kinds() {
                for kind in kinds {
                    if kind == 0 || kind == 3 || kind == 10002 {
                        return self.indexer_relays.clone();
                    }
                }
            }
            // Otherwise use default relays
            return self.default_relays.clone();
        }

        // Make a single query for all pubkeys' kind 10002 events
        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![10002]);
        filter.authors = Some(pubkeys_to_check);
        // No limit since we want the latest 10002 for each author

        if let Ok(result) = self.query_events_with_filter(filter) {
            // Group events by pubkey and keep only the latest one for each
            let mut latest_events: FxHashMap<String, Vec<u8>> = FxHashMap::default();

            for event_bytes in result.events {
                if let Some(event) = Self::extract_parsed_event(&event_bytes) {
                    let pubkey = event.pubkey().to_string();

                    // Check if we already have an event for this pubkey
                    if let Some(existing) = latest_events.get(&pubkey) {
                        if let Some(existing_event) = Self::extract_parsed_event(existing) {
                            // Keep the newer event
                            if event.created_at() > existing_event.created_at() {
                                latest_events.insert(pubkey, event_bytes);
                            }
                        }
                    } else {
                        latest_events.insert(pubkey, event_bytes);
                    }
                }
            }

            // Process the latest events to extract relays
            for (pubkey, event_bytes) in latest_events {
                if let Some(event) = Self::extract_parsed_event(&event_bytes) {
                    if let Some(kind10002) = event.parsed_as_kind_10002_parsed() {
                        // Determine if this pubkey needs read or write relays
                        let is_author = authors_set.contains(&pubkey);

                        for relay in kind10002.relays() {
                            // If pubkey is in authors filter, we need write relays (they're posting)
                            // Otherwise, we need read relays (we're reading their events)
                            if is_author {
                                if relay.write() {
                                    let url = relay.url().to_string();
                                    *relay_counts.entry(url).or_insert(0) += 1;
                                }
                            } else {
                                if relay.read() {
                                    let url = relay.url().to_string();
                                    *relay_counts.entry(url).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        // If no relays found from 10002 events, use fallback based on kind
        if relay_counts.is_empty() {
            // Check if the request is for indexer kinds (0, 3, 10002)
            if let Some(kinds) = fb_req.kinds() {
                for kind in kinds {
                    if kind == 0 || kind == 3 || kind == 10002 {
                        return self.indexer_relays.clone();
                    }
                }
            }
            // Otherwise use default relays
            self.default_relays.clone()
        } else {
            // Sort relays by count (descending) and then by URL (for stability)
            let mut relay_vec: Vec<(String, usize)> = relay_counts.into_iter().collect();
            relay_vec.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            // Return just the relay URLs in sorted order, limited to 15 relays
            relay_vec.into_iter().take(15).map(|(url, _)| url).collect()
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

    async fn join_all_seq<I, F, T>(futs: I) -> Vec<T>
    where
        I: IntoIterator<Item = F>,
        F: Future<Output = T>,
    {
        let mut results = Vec::new();
        for fut in futs {
            results.push(fut.await);
        }
        results
    }

    pub async fn determine_target_relays(&self, event: NostrEvent<'_>) -> Result<Vec<String>> {
        let mut relay_set = FxHashSet::default();
        let mut write_pubkeys = Vec::new();
        let mut read_pubkeys = Vec::new();

        // Always add the event author's pubkey as a write pubkey
        write_pubkeys.push(event.pubkey().to_string());

        // Skip extracting mentioned pubkeys for kind 3 (contact list) events
        if event.kind() != 3 && event.kind() < 10000 {
            let tags = event.tags();
            for i in 0..tags.len() {
                let tag = tags.get(i);
                if let Some(tag_vec) = tag.items() {
                    if tag_vec.len() >= 2 {
                        let tag_kind = tag_vec.get(0);
                        if tag_kind == "p" {
                            let tag_value = tag_vec.get(1);
                            read_pubkeys.push(tag_value.to_string());
                        }
                    }
                }
            }
        }

        // Get relays for all mentioned pubkeys (read relays)
        let read_tasks: Vec<_> = read_pubkeys
            .into_iter()
            .map(|pubkey| async move { self.get_read_relays(&pubkey).unwrap_or_default() })
            .collect();

        // Get relays for author pubkeys (write relays)
        let write_tasks: Vec<_> = write_pubkeys
            .into_iter()
            .map(|pubkey| async move { self.get_write_relays(&pubkey).unwrap_or_default() })
            .collect();

        // Wait for all tasks to complete
        let read_results = Self::join_all_seq(read_tasks).await;
        let write_results = Self::join_all_seq(write_tasks).await;

        // Collect all relay URLs
        for relays in read_results.into_iter().chain(write_results.into_iter()) {
            for relay in relays {
                relay_set.insert(relay);
            }
        }

        Ok(relay_set.into_iter().collect())
    }
}
