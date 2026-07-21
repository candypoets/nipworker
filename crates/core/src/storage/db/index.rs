use crate::generated::nostr::fb::{self, NostrEvent, ParsedEvent, Request, WorkerMessage};
use crate::platform::now_millis;
use crate::storage::db::sharded_storage::ShardedRingBufferStorage;
use crate::storage::db::types::{
    DatabaseConfig, DatabaseError, DatabaseIndexes, EventKey, EventRecord, EventStorage,
    QueryFilter, QueryResult, Tombstones,
};
use crate::types::nostr::EVENT_DELETION;
use rustc_hash::{FxHashMap, FxHashSet};

type Result<T> = std::result::Result<T, DatabaseError>;

/// A candidate event-key set for query evaluation. Single-value indexed fields
/// borrow the index set directly (the common case avoids cloning the whole
/// set); multi-value fields union into an owned set.
enum CandidateSet<'a> {
    Owned(FxHashSet<EventKey>),
    Borrowed(&'a FxHashSet<EventKey>),
}

impl CandidateSet<'_> {
    fn as_set(&self) -> &FxHashSet<EventKey> {
        match self {
            CandidateSet::Owned(set) => set,
            CandidateSet::Borrowed(set) => set,
        }
    }
}

/// Outcome of gathering candidates for one indexed filter field.
enum GatheredCandidates<'a> {
    /// Field not present in the filter.
    Absent,
    /// Field present but no events match - the whole query result is empty.
    Empty,
    /// Field present with matching events.
    Set(CandidateSet<'a>),
}

use std::cell::RefCell;
use std::future::Future;
use std::rc::Rc;
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
    /// NIP-09 deletion tombstones, resolved to index keys at kind-5 ingest
    tombstones: Rc<RefCell<Tombstones>>,
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
            tombstones: Rc::new(RefCell::new(Tombstones::default())),
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

        // Clear existing indexes and tombstones (EventKeys are
        // session-sequential; the deletion WAL is replayed after rebuild)
        self.indexes.clear();
        self.tombstones.borrow_mut().clear();

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

    /// Get a reference to the underlying storage
    pub fn storage(&self) -> &S {
        &self.storage
    }

    /// Rebuild query indexes from the currently loaded storage contents.
    ///
    /// This is used by platform wrappers that hydrate the backing storage after
    /// core initialization, such as WASM IndexedDB persistence.
    pub fn rebuild_indexes_from_storage(&self) -> Result<()> {
        self.indexes.clear();
        self.tombstones.borrow_mut().clear();

        let events = self.storage.load_events()?;
        if !events.is_empty() {
            info!("Rebuilding indexes from {} storage events", events.len());
            self.build_indexes_from_events(events)?;
        }

        Ok(())
    }

    pub async fn add_worker_message_bytes(&self, bytes: &[u8]) -> Result<()> {
        let sharded = self
            .storage
            .as_any()
            .downcast_ref::<ShardedRingBufferStorage>();

        // Try WorkerMessage first (SaveToDbPipe sends this format)
        if let Ok(worker_msg) = flatbuffers::root::<WorkerMessage>(bytes) {
            // Check if it's a ParsedEvent
            if let Some(parsed) = worker_msg.content_as_parsed_event() {
                let offset = if let Some(sharded) = sharded {
                    sharded
                        .add_event_for_kind(parsed.kind() as u32, bytes)
                        .await?
                } else {
                    self.storage.add_event_data(bytes).await?
                };

                self.index_parsed_event(parsed, offset);
                return Ok(());
            }

            // Check if it's a NostrEvent
            if let Some(nostr) = worker_msg.content_as_nostr_event() {
                let offset = if let Some(sharded) = sharded {
                    sharded
                        .add_event_for_kind(nostr.kind() as u32, bytes)
                        .await?
                } else {
                    self.storage.add_event_data(bytes).await?
                };

                self.index_nostr_event(nostr, offset);
                return Ok(());
            }

            // Other message types - skip
            return Ok(());
        }

        // Try raw ParsedEvent (backward compat)
        if let Ok(parsed) = flatbuffers::root::<ParsedEvent>(bytes) {
            let offset = if let Some(sharded) = sharded {
                sharded
                    .add_event_for_kind(parsed.kind() as u32, bytes)
                    .await?
            } else {
                self.storage.add_event_data(bytes).await?
            };

            self.index_parsed_event(parsed, offset);
            return Ok(());
        }

        // Fallback: raw NostrEvent
        if let Ok(nostr) = flatbuffers::root::<NostrEvent>(bytes) {
            let offset = if let Some(sharded) = sharded {
                sharded
                    .add_event_for_kind(nostr.kind() as u32, bytes)
                    .await?
            } else {
                self.storage.add_event_data(bytes).await?
            };

            self.index_nostr_event(nostr, offset);
            return Ok(());
        }

        Err(DatabaseError::StorageError(
            "FB decode error: expected WorkerMessage, ParsedEvent or NostrEvent".to_string(),
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
        let event_id = parsed.id().to_string();
        // Avoid re-storing duplicates (common when cached events re-enter pipelines with SaveToDb).
        if let Some(existing_offset) = self
            .indexes
            .events_by_id
            .borrow()
            .get(&event_id)
            .map(|record| record.offset)
        {
            // Only skip when the indexed offset still points to a readable event.
            // Offsets can become stale after ring-buffer eviction. This is a cheap
            // bounds check - it does NOT copy the existing event bytes.
            if self.storage.contains_offset(existing_offset) {
                return Ok(());
            }
        }

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
        let event_id = event.id().to_string();
        // Avoid re-storing duplicates (common when cached events re-enter pipelines with SaveToDb).
        if let Some(existing_offset) = self
            .indexes
            .events_by_id
            .borrow()
            .get(&event_id)
            .map(|record| record.offset)
        {
            // Only skip when the indexed offset still points to a readable event.
            // Offsets can become stale after ring-buffer eviction. This is a cheap
            // bounds check - it does NOT copy the existing event bytes.
            if self.storage.contains_offset(existing_offset) {
                return Ok(());
            }
        }

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
            let mut q_tags: Vec<String> = Vec::new();

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
                            "q" => q_tags.extend(values),
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
            if !q_tags.is_empty() {
                f.q_tags = Some(q_tags);
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

        info!(
            "build_indexes_from_events: loaded {} events from storage",
            raw_events.len()
        );

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

        info!(
            "build_indexes_from_events: {} WorkerMessage format, {} legacy format",
            worker_message_count, legacy_count
        );

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

        info!(
            "build_indexes_from_events: indexed {} events",
            indexed_count
        );
        Ok(())
    }

    /// Try to extract a ParsedEvent from raw bytes
    /// Handles both WorkerMessage wrapper (new format) and direct event (legacy)
    fn extract_parsed_event(bytes: &[u8]) -> Option<ParsedEvent<'_>> {
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

    /// Extract created_at regardless of format (ParsedEvent or NostrEvent)
    /// Handles both WorkerMessage wrapper (new format) and direct event (legacy)
    fn extract_created_at(bytes: &[u8]) -> Option<u32> {
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
    /// NIP-09 (kind 5) deletion processing. Resolves referenced events to
    /// index keys once, at ingest time, so the query hot path only probes
    /// `Tombstones::deleted_keys`. All validation uses indexes — no event-byte
    /// reads: an e-tag deletion is valid only if the target key is in the
    /// author's own pubkey set, and an a-tag deletion carries its author in
    /// the address string itself.
    fn process_deletion_tags(
        &self,
        author: &str,
        deletion_created_at: u32,
        tags: flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<fb::StringVec<'_>>>,
    ) {
        let mut tombstones = self.tombstones.borrow_mut();
        for i in 0..tags.len() {
            let tag = tags.get(i);
            let Some(items) = tag.items() else { continue };
            if items.len() < 2 {
                continue;
            }
            let tag_value = items.get(1);
            match items.get(0) {
                "e" => {
                    let record = self.indexes.events_by_id.borrow().get(tag_value).copied();
                    match record {
                        Some(record) => {
                            let authored = self
                                .indexes
                                .events_by_pubkey
                                .borrow()
                                .get(author)
                                .is_some_and(|keys| keys.contains(&record.key));
                            if authored {
                                tombstones.deleted_keys.insert(record.key);
                            }
                            // Author mismatch: drop the reference entirely.
                        }
                        None => {
                            // Target not cached (yet); validate author at arrival.
                            tombstones
                                .pending_ids
                                .entry(tag_value.to_string())
                                .and_modify(|slot| {
                                    if deletion_created_at > slot.1 {
                                        *slot = (author.to_string(), deletion_created_at);
                                    }
                                })
                                .or_insert_with(|| (author.to_string(), deletion_created_at));
                        }
                    }
                }
                "a" => {
                    // a-tag: "kind:pubkey:d" — validate against the address author.
                    let mut parts = tag_value.splitn(3, ':');
                    let (Some(kind_str), Some(addr_author), Some(d)) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        continue;
                    };
                    if addr_author != author {
                        continue;
                    }
                    let Ok(kind) = kind_str.parse::<u16>() else {
                        continue;
                    };

                    {
                        let d_map = self.indexes.events_by_d_tag.borrow();
                        let kind_map = self.indexes.events_by_kind.borrow();
                        let pub_map = self.indexes.events_by_pubkey.borrow();
                        if let (Some(d_set), Some(kind_set), Some(pub_set)) =
                            (d_map.get(d), kind_map.get(&kind), pub_map.get(addr_author))
                        {
                            // Intersect d ∩ kind ∩ pubkey, iterating the smallest set.
                            let mut sets = [d_set, kind_set, pub_set];
                            sets.sort_by_key(|set| set.len());
                            let (driver, others) = sets.split_first().unwrap();
                            let events_by_key = self.indexes.events_by_key.borrow();
                            for key in driver.iter() {
                                if !others.iter().all(|set| set.contains(key)) {
                                    continue;
                                }
                                if let Some(record) = events_by_key.get(key) {
                                    // Freshness guard: the deletion only covers
                                    // versions created at/before it.
                                    if record.created_at <= deletion_created_at {
                                        tombstones.deleted_keys.insert(*key);
                                    }
                                }
                            }
                        }
                    }

                    tombstones
                        .deleted_addresses
                        .entry(tag_value.to_string())
                        .and_modify(|ts| *ts = (*ts).max(deletion_created_at))
                        .or_insert(deletion_created_at);
                }
                _ => {}
            }
        }
    }

    /// Check a freshly indexed event against recorded tombstones. Runs at the
    /// end of indexing so deletions apply to events that arrive after their
    /// kind 5 (out-of-order delivery, cache replay from another relay).
    #[allow(clippy::too_many_arguments)]
    fn apply_tombstones_to_event(
        &self,
        event_key: EventKey,
        event_id: &str,
        author: &str,
        kind: u16,
        created_at: u32,
        d_tag: Option<&str>,
    ) {
        let mut tombstones = self.tombstones.borrow_mut();
        if tombstones.is_empty() {
            return;
        }
        if let Some((deletion_author, _)) = tombstones.pending_ids.get(event_id) {
            if deletion_author == author {
                tombstones.deleted_keys.insert(event_key);
                return;
            }
        }
        // Parameterized replaceable events (NIP-33) are deletable by address.
        if (30000..40000).contains(&kind) {
            if let Some(d) = d_tag {
                let address = format!("{}:{}:{}", kind, author, d);
                if let Some(deletion_ts) = tombstones.deleted_addresses.get(&address) {
                    if created_at <= *deletion_ts {
                        tombstones.deleted_keys.insert(event_key);
                    }
                }
            }
        }
    }

    /// Apply NIP-09 deletions from a stored WorkerMessage without persisting
    /// the event. Used to replay the deletion WAL after an index rebuild.
    /// Returns the deletion event's id when the bytes held a kind 5.
    pub fn apply_deletions_from_bytes(&self, bytes: &[u8]) -> Option<String> {
        let wm = flatbuffers::root::<WorkerMessage>(bytes).ok()?;
        match wm.content_type() {
            fb::Message::ParsedEvent => {
                let parsed = wm.content_as_parsed_event()?;
                if parsed.kind() == EVENT_DELETION {
                    self.process_deletion_tags(parsed.pubkey(), parsed.created_at(), parsed.tags());
                    return Some(parsed.id().to_string());
                }
                None
            }
            fb::Message::NostrEvent => {
                let event = wm.content_as_nostr_event()?;
                if event.kind() == EVENT_DELETION {
                    self.process_deletion_tags(
                        event.pubkey(),
                        event.created_at().max(0) as u32,
                        event.tags(),
                    );
                    return Some(event.id().to_string());
                }
                None
            }
            _ => None,
        }
    }

    /// Number of cached events currently suppressed by NIP-09 deletions.
    pub fn deleted_count(&self) -> usize {
        self.tombstones.borrow().deleted_keys.len()
    }

    /// Extract the first `d` tag value (NIP-33 identifier), if any.
    fn first_d_tag<'a>(
        tags: &flatbuffers::Vector<'a, flatbuffers::ForwardsUOffset<fb::StringVec<'a>>>,
    ) -> Option<&'a str> {
        for i in 0..tags.len() {
            let tag = tags.get(i);
            if let Some(items) = tag.items() {
                if items.len() >= 2 && items.get(0) == "d" {
                    return Some(items.get(1));
                }
            }
        }
        None
    }

    fn index_parsed_event(&self, event: ParsedEvent<'_>, offset: u64) {
        let event_id = event.id();
        let event_key = self
            .indexes
            .upsert_event_record(event_id, offset, event.created_at());

        // Kind
        self.indexes
            .events_by_kind
            .borrow_mut()
            .entry(event.kind())
            .or_insert_with(FxHashSet::default)
            .insert(event_key);

        // Pubkey
        self.indexes
            .events_by_pubkey
            .borrow_mut()
            .entry(event.pubkey().to_string())
            .or_insert_with(FxHashSet::default)
            .insert(event_key);

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
                                .insert(event_key);
                        }
                        "E" => {
                            // Uppercase E - NIP-22 "E" tag (event id reference)
                            self.indexes
                                .events_by_E_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "p" => {
                            self.indexes
                                .events_by_p_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "P" => {
                            // Uppercase P - NIP-22 "P" tag (pubkey reference)
                            self.indexes
                                .events_by_P_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "a" => {
                            self.indexes
                                .events_by_a_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "d" => {
                            self.indexes
                                .events_by_d_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "q" => {
                            // q tag (quote/citation)
                            self.indexes
                                .events_by_q_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        _ => {}
                    }
                }
            }
        }

        self.apply_tombstones_to_event(
            event_key,
            event_id,
            event.pubkey(),
            event.kind(),
            event.created_at(),
            Self::first_d_tag(&tags),
        );
        if event.kind() == EVENT_DELETION {
            self.process_deletion_tags(event.pubkey(), event.created_at(), tags);
        }
    }

    fn index_nostr_event(&self, event: NostrEvent<'_>, offset: u64) {
        let event_id = event.id();
        let event_key =
            self.indexes
                .upsert_event_record(event_id, offset, event.created_at().max(0) as u32);

        // Kind
        self.indexes
            .events_by_kind
            .borrow_mut()
            .entry(event.kind())
            .or_insert_with(FxHashSet::default)
            .insert(event_key);

        // Pubkey
        self.indexes
            .events_by_pubkey
            .borrow_mut()
            .entry(event.pubkey().to_string())
            .or_insert_with(FxHashSet::default)
            .insert(event_key);

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
                                .insert(event_key);
                        }
                        "E" => {
                            // Uppercase E - NIP-22 "E" tag (event id reference)
                            self.indexes
                                .events_by_E_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "p" => {
                            self.indexes
                                .events_by_p_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "P" => {
                            // Uppercase P - NIP-22 "P" tag (pubkey reference)
                            self.indexes
                                .events_by_P_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "a" => {
                            self.indexes
                                .events_by_a_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "d" => {
                            self.indexes
                                .events_by_d_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        "q" => {
                            // q tag (quote/citation)
                            self.indexes
                                .events_by_q_tag
                                .borrow_mut()
                                .entry(tag_value.to_string())
                                .or_insert_with(FxHashSet::default)
                                .insert(event_key);
                        }
                        _ => {}
                    }
                }
            }
        }

        let created_at = event.created_at().max(0) as u32;
        self.apply_tombstones_to_event(
            event_key,
            event_id,
            event.pubkey(),
            event.kind(),
            created_at,
            Self::first_d_tag(&tags),
        );
        if event.kind() == EVENT_DELETION {
            self.process_deletion_tags(event.pubkey(), created_at, tags);
        }
    }

    fn dedup_strings(values: &mut Option<Vec<String>>) {
        if let Some(values) = values {
            if values.len() < 2 {
                return;
            }
            let mut seen = FxHashSet::default();
            values.retain(|value| seen.insert(value.clone()));
        }
    }

    fn dedup_kinds(values: &mut Option<Vec<u16>>) {
        if let Some(values) = values {
            if values.len() < 2 {
                return;
            }
            let mut seen = FxHashSet::default();
            values.retain(|value| seen.insert(*value));
        }
    }

    #[allow(non_snake_case)]
    fn normalize_query_filter(filter: &mut QueryFilter) {
        Self::dedup_strings(&mut filter.ids);
        Self::dedup_strings(&mut filter.authors);
        Self::dedup_kinds(&mut filter.kinds);
        Self::dedup_strings(&mut filter.e_tags);
        Self::dedup_strings(&mut filter.E_tags);
        Self::dedup_strings(&mut filter.p_tags);
        Self::dedup_strings(&mut filter.P_tags);
        Self::dedup_strings(&mut filter.a_tags);
        Self::dedup_strings(&mut filter.d_tags);
        Self::dedup_strings(&mut filter.q_tags);
    }

    fn empty_query_result(start_time: u64) -> QueryResult {
        QueryResult {
            events: Vec::new(),
            total_found: 0,
            has_more: false,
            query_time_ms: now_millis() - start_time,
        }
    }

    /// Gather the candidate event-key set for one indexed filter field.
    /// Borrows the index set when the field has a single value (no clone);
    /// unions into an owned set for multiple values.
    fn gather_candidates<'a, K: Eq + std::hash::Hash>(
        values: &Option<Vec<K>>,
        index: &'a FxHashMap<K, FxHashSet<EventKey>>,
    ) -> GatheredCandidates<'a> {
        let Some(values) = values else {
            return GatheredCandidates::Absent;
        };

        if values.len() == 1 {
            return match index.get(&values[0]) {
                Some(set) if !set.is_empty() => {
                    GatheredCandidates::Set(CandidateSet::Borrowed(set))
                }
                _ => GatheredCandidates::Empty,
            };
        }

        let mut union = FxHashSet::default();
        for value in values {
            if let Some(set) = index.get(value) {
                union.extend(set.iter().copied());
            }
        }
        if union.is_empty() {
            GatheredCandidates::Empty
        } else {
            GatheredCandidates::Set(CandidateSet::Owned(union))
        }
    }

    #[allow(non_snake_case)]
    pub fn query_events_with_filter(&self, filter: QueryFilter) -> Result<QueryResult> {
        self.query_events_with_filter_inner(filter, true)
    }

    #[allow(non_snake_case)]
    fn query_events_with_filter_inner(
        &self,
        mut filter: QueryFilter,
        sort_results: bool,
    ) -> Result<QueryResult> {
        let start_time = now_millis();
        Self::normalize_query_filter(&mut filter);

        // Borrow all index maps up front (shared borrows on distinct RefCells)
        // so single-value candidate sets can borrow directly from the indexes.
        let events_by_id = self.indexes.events_by_id.borrow();
        let events_by_kind = self.indexes.events_by_kind.borrow();
        let events_by_pubkey = self.indexes.events_by_pubkey.borrow();
        let events_by_e_tag = self.indexes.events_by_e_tag.borrow();
        let events_by_E_tag = self.indexes.events_by_E_tag.borrow();
        let events_by_p_tag = self.indexes.events_by_p_tag.borrow();
        let events_by_P_tag = self.indexes.events_by_P_tag.borrow();
        let events_by_a_tag = self.indexes.events_by_a_tag.borrow();
        let events_by_d_tag = self.indexes.events_by_d_tag.borrow();
        let events_by_q_tag = self.indexes.events_by_q_tag.borrow();

        macro_rules! push_candidates {
            ($sets:ident, $start:expr, $values:expr, $index:expr) => {
                match Self::gather_candidates($values, $index) {
                    GatheredCandidates::Absent => {}
                    GatheredCandidates::Empty => {
                        return Ok(Self::empty_query_result($start));
                    }
                    GatheredCandidates::Set(set) => $sets.push(set),
                }
            };
        }

        // Start with candidate sets from indexed fields
        let mut candidate_sets: Vec<CandidateSet<'_>> = Vec::new();

        // Filter by IDs (most specific)
        if let Some(ids) = &filter.ids {
            let mut id_set = FxHashSet::default();
            for id in ids {
                if let Some(record) = events_by_id.get(id) {
                    id_set.insert(record.key);
                }
            }
            if id_set.is_empty() {
                return Ok(Self::empty_query_result(start_time));
            }
            candidate_sets.push(CandidateSet::Owned(id_set));
        }

        // Filter by kinds
        push_candidates!(candidate_sets, start_time, &filter.kinds, &events_by_kind);

        // Filter by authors
        push_candidates!(
            candidate_sets,
            start_time,
            &filter.authors,
            &events_by_pubkey
        );

        // Filter by e_tags
        push_candidates!(candidate_sets, start_time, &filter.e_tags, &events_by_e_tag);

        // Filter by E_tags (uppercase - NIP-22)
        push_candidates!(candidate_sets, start_time, &filter.E_tags, &events_by_E_tag);

        // Filter by p_tags
        push_candidates!(candidate_sets, start_time, &filter.p_tags, &events_by_p_tag);

        // Filter by P_tags (uppercase - NIP-22)
        push_candidates!(candidate_sets, start_time, &filter.P_tags, &events_by_P_tag);

        // Filter by a_tags
        push_candidates!(candidate_sets, start_time, &filter.a_tags, &events_by_a_tag);

        // Filter by d_tags
        push_candidates!(candidate_sets, start_time, &filter.d_tags, &events_by_d_tag);

        // Filter by q_tags (quote/citation)
        push_candidates!(candidate_sets, start_time, &filter.q_tags, &events_by_q_tag);

        // Apply non-indexed filters and collect surviving (created_at, offset)
        // pairs. created_at comes from the index record, so since/until pruning
        // happens WITHOUT reading event bytes from storage.
        let use_full_scan = candidate_sets.is_empty();
        let events_by_key = self.indexes.events_by_key.borrow();
        let tombstones = self.tombstones.borrow();
        let has_deletions = !tombstones.deleted_keys.is_empty();
        let mut survivors: Vec<(u32, u64)> = Vec::new();

        let consider = |record: EventRecord, survivors: &mut Vec<(u32, u64)>| {
            // NIP-09: skip tombstoned events (single FxHashSet probe, guarded
            // so it costs nothing when no deletions have been seen).
            if has_deletions && tombstones.deleted_keys.contains(&record.key) {
                return;
            }
            // Time range filters (from the index record, no byte reads)
            if let Some(since) = filter.since {
                if record.created_at < since {
                    return;
                }
            }
            if let Some(until) = filter.until {
                if record.created_at > until {
                    return;
                }
            }
            // Skip offsets evicted from the ring buffer. This is a cheap bounds
            // check - it does NOT copy event bytes like get_event would.
            if !self.storage.contains_offset(record.offset) {
                return;
            }
            survivors.push((record.created_at, record.offset));
        };

        if use_full_scan {
            for record in events_by_key.values().copied() {
                consider(record, &mut survivors);
            }
        } else {
            // Intersect: iterate the smallest candidate set, probe the others.
            let mut order: Vec<usize> = (0..candidate_sets.len()).collect();
            order.sort_by_key(|&i| candidate_sets[i].as_set().len());
            let (driver_idx, remaining_idxs) = order.split_first().unwrap();
            let driver_set = candidate_sets[*driver_idx].as_set();
            'candidates: for event_key in driver_set {
                for &idx in remaining_idxs {
                    if !candidate_sets[idx].as_set().contains(event_key) {
                        continue 'candidates;
                    }
                }
                if let Some(record) = events_by_key.get(event_key).copied() {
                    consider(record, &mut survivors);
                }
            }
        }
        drop(events_by_key);

        let total_found = survivors.len();

        // Apply limit as a top-k selection by created_at (partial sort) so we
        // never read event bytes for candidates beyond the limit.
        if let Some(limit) = filter.limit {
            if limit == 0 {
                survivors.clear();
            } else if survivors.len() > limit {
                survivors.select_nth_unstable_by(limit - 1, |a, b| b.0.cmp(&a.0));
                survivors.truncate(limit);
            }
        }

        if sort_results {
            survivors.sort_by(|a, b| b.0.cmp(&a.0));
        }

        // Only now read event bytes, and only for the surviving (<= limit)
        // candidates.
        let mut results: Vec<Vec<u8>> = Vec::with_capacity(survivors.len());
        for (_, offset) in survivors {
            if let Ok(Some(bytes)) = self.storage.get_event(offset) {
                results.push(bytes);
            }
        }

        let has_more = filter.limit.is_some_and(|limit| total_found > limit);

        let query_time = now_millis() - start_time;

        // Log slow queries (>1ms) for debugging
        if query_time > 1 {
            info!(
                "[NostrDB] Slow query detected: {}ms | filter=[ids={}, kinds={}, authors={}, since={:?}, until={:?}, limit={:?}] | total_found={}",
                query_time,
                filter.ids.as_ref().map(|v| v.len()).unwrap_or(0),
                filter.kinds.as_ref().map(|v| v.len()).unwrap_or(0),
                filter.authors.as_ref().map(|v| v.len()).unwrap_or(0),
                filter.since, filter.until, filter.limit,
                total_found
            );
        }

        Ok(QueryResult {
            events: results,
            total_found,
            has_more,
            query_time_ms: query_time,
        })
    }

    /// Query events using the internal filter format
    pub fn query_events(&self, fb_req: &Request<'_>) -> Result<QueryResult> {
        let filter = Self::query_filter_from_fb_request(fb_req)?;
        info!(
            "query_events: filter kinds={:?}, authors={:?}, limit={:?}",
            filter.kinds, filter.authors, filter.limit
        );
        self.query_events_with_filter(filter)
    }

    pub async fn query_events_and_requests(
        &self,
        fb_reqs: Option<flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<Request<'_>>>>,
    ) -> Result<(Vec<usize>, Vec<Vec<u8>>)> {
        let mut remaining_indices: Vec<usize> = Vec::new();
        let mut all_events: Vec<Vec<u8>> = Vec::new();

        if let Some(vec) = fb_reqs {
            for i in 0..vec.len() {
                let req = vec.get(i);

                // Respect no_cache in request
                if req.no_cache() {
                    remaining_indices.push(i);
                    continue;
                }

                let filter = match Self::query_filter_from_fb_request(&req) {
                    Ok(filter) => filter,
                    Err(e) => {
                        warn!("query filter conversion failed for request {}: {}", i, e);
                        remaining_indices.push(i);
                        continue;
                    }
                };

                let result = match self.query_events_with_filter_inner(filter, false) {
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

                // If cache_only is true -> never forward (skip network REQ)
                // If cache_first is true and we have results -> skip network REQ
                // Otherwise -> forward to network
                if req.cache_only() {
                    // skip network REQ entirely
                } else if !req.cache_first() || result.total_found == 0 {
                    remaining_indices.push(i);
                }
            }
        }

        // Sort newest-first (same as other paths) — pre-extract to avoid O(n log n) flatbuffer parses
        let mut with_time: Vec<(u32, Vec<u8>)> = all_events
            .into_iter()
            .map(|b| (Self::extract_created_at(&b).unwrap_or_default(), b))
            .collect();
        with_time.sort_by(|a, b| b.0.cmp(&a.0));
        all_events = with_time.into_iter().map(|(_, b)| b).collect();

        Ok((remaining_indices, all_events))
    }

    /// Get a single event by ID
    pub fn get_event(&self, id: &str) -> Option<Vec<u8>> {
        let offset = self
            .indexes
            .events_by_id
            .borrow()
            .get(id)
            .map(|record| record.offset)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::db::ring_buffer::RingBufferStorage;
    use flatbuffers::FlatBufferBuilder;
    use std::cell::Cell;

    /// Storage wrapper that counts byte reads, proving which query stages
    /// avoid copying event bytes out of the ring buffer.
    struct CountingStorage {
        inner: RingBufferStorage,
        get_calls: Cell<usize>,
        contains_calls: Cell<usize>,
    }

    impl CountingStorage {
        fn new(max_buffer_size: usize) -> Self {
            Self {
                inner: RingBufferStorage::new(
                    "test-db".to_string(),
                    "rb:test".to_string(),
                    max_buffer_size,
                    DatabaseConfig::default(),
                ),
                get_calls: Cell::new(0),
                contains_calls: Cell::new(0),
            }
        }

        fn reset_counters(&self) {
            self.get_calls.set(0);
            self.contains_calls.set(0);
        }
    }

    impl EventStorage for CountingStorage {
        async fn initialize_storage(&self) -> std::result::Result<(), DatabaseError> {
            self.inner.initialize_storage().await
        }

        async fn add_event_data(
            &self,
            event_data: &[u8],
        ) -> std::result::Result<u64, DatabaseError> {
            self.inner.add_event_data(event_data).await
        }

        fn get_event(
            &self,
            event_offset: u64,
        ) -> std::result::Result<Option<Vec<u8>>, DatabaseError> {
            self.get_calls.set(self.get_calls.get() + 1);
            self.inner.get_event(event_offset)
        }

        fn contains_offset(&self, event_offset: u64) -> bool {
            self.contains_calls.set(self.contains_calls.get() + 1);
            self.inner.contains_offset(event_offset)
        }

        fn load_events(&self) -> std::result::Result<Vec<u64>, DatabaseError> {
            self.inner.load_events()
        }

        async fn clear_storage(&self) -> std::result::Result<(), DatabaseError> {
            self.inner.clear_storage().await
        }

        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
    }

    fn new_test_db(max_buffer_size: usize) -> NostrDB<CountingStorage> {
        NostrDB {
            indexes: DatabaseIndexes::new(),
            storage: CountingStorage::new(max_buffer_size),
            is_initialized: Arc::new(RwLock::new(false)),
            tombstones: Rc::new(RefCell::new(Tombstones::default())),
            default_relays: vec![],
            indexer_relays: vec![],
        }
    }

    fn build_parsed_worker_message(
        id: &str,
        pubkey: &str,
        kind: u16,
        created_at: u32,
        tags: &[&[&str]],
    ) -> Vec<u8> {
        let mut builder = FlatBufferBuilder::new();
        let id_off = builder.create_string(id);
        let pubkey_off = builder.create_string(pubkey);

        let tag_offsets: Vec<_> = tags
            .iter()
            .map(|tag| {
                let items: Vec<_> = tag.iter().map(|s| builder.create_string(s)).collect();
                let items = builder.create_vector(&items);
                fb::StringVec::create(&mut builder, &fb::StringVecArgs { items: Some(items) })
            })
            .collect();
        let tags_vec = builder.create_vector(&tag_offsets);

        let parsed = fb::ParsedEvent::create(
            &mut builder,
            &fb::ParsedEventArgs {
                id: Some(id_off),
                pubkey: Some(pubkey_off),
                kind,
                created_at,
                tags: Some(tags_vec),
                ..Default::default()
            },
        );
        let sub_id_off = builder.create_string("save_to_db");
        let message = fb::WorkerMessage::create(
            &mut builder,
            &fb::WorkerMessageArgs {
                sub_id: Some(sub_id_off),
                content_type: fb::Message::ParsedEvent,
                content: Some(parsed.as_union_value()),
                ..Default::default()
            },
        );
        builder.finish(message, None);
        builder.finished_data().to_vec()
    }

    fn event_id(index: usize) -> String {
        format!("{:064x}", index + 1)
    }

    fn pubkey_id(index: usize) -> String {
        format!("p{:063x}", index)
    }

    fn result_created_ats(result: &QueryResult) -> Vec<u32> {
        result
            .events
            .iter()
            .map(|b| NostrDB::<CountingStorage>::extract_created_at(b).unwrap())
            .collect()
    }

    #[tokio::test]
    async fn query_limit_reads_only_top_k_events() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        // 50 events of kind 1 with created_at 1..=50
        for i in 0..50u32 {
            let bytes =
                build_parsed_worker_message(&event_id(i as usize), &pubkey_id(0), 1, i + 1, &[]);
            db.add_worker_message_bytes(&bytes).await.unwrap();
        }
        db.storage.reset_counters();

        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![1]);
        filter.limit = Some(10);

        let result = db.query_events_with_filter(filter).unwrap();

        assert_eq!(result.total_found, 50);
        assert!(result.has_more);
        assert_eq!(result.events.len(), 10);
        // Sorted newest-first: the 10 newest events (created_at 50 down to 41)
        assert_eq!(
            result_created_ats(&result),
            (41..=50).rev().collect::<Vec<_>>()
        );
        // Only the 10 returned events were read from storage - not all 50.
        assert_eq!(db.storage.get_calls.get(), 10);
    }

    #[tokio::test]
    async fn query_since_until_prunes_without_byte_reads() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        for i in 0..20u32 {
            let bytes =
                build_parsed_worker_message(&event_id(i as usize), &pubkey_id(0), 1, i + 1, &[]);
            db.add_worker_message_bytes(&bytes).await.unwrap();
        }
        db.storage.reset_counters();

        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![1]);
        filter.since = Some(5);
        filter.until = Some(10);

        let result = db.query_events_with_filter(filter).unwrap();

        assert_eq!(result.total_found, 6);
        assert!(!result.has_more);
        // created_at 10 down to 5, newest first
        assert_eq!(result_created_ats(&result), vec![10, 9, 8, 7, 6, 5]);
        // Pruning happened on index records: only the 6 survivors were read.
        assert_eq!(db.storage.get_calls.get(), 6);
    }

    #[tokio::test]
    async fn query_multi_field_intersection_matches_smallest_driver() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        // Author A: 5 kind-1 events; author B: 5 kind-1 + 5 kind-6 events
        for i in 0..5u32 {
            let a =
                build_parsed_worker_message(&event_id(i as usize), &pubkey_id(0), 1, i + 1, &[]);
            db.add_worker_message_bytes(&a).await.unwrap();
            let b1 = build_parsed_worker_message(
                &event_id(10 + i as usize),
                &pubkey_id(1),
                1,
                i + 1,
                &[],
            );
            db.add_worker_message_bytes(&b1).await.unwrap();
            let b6 = build_parsed_worker_message(
                &event_id(20 + i as usize),
                &pubkey_id(1),
                6,
                i + 1,
                &[],
            );
            db.add_worker_message_bytes(&b6).await.unwrap();
        }

        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![1]);
        filter.authors = Some(vec![pubkey_id(1)]);

        let result = db.query_events_with_filter(filter).unwrap();

        assert_eq!(result.total_found, 5);
        assert_eq!(result.events.len(), 5);
        // Only author B's kind-1 events
        for bytes in &result.events {
            let event =
                NostrDB::<CountingStorage>::extract_parsed_event(bytes).expect("parsed event");
            assert_eq!(event.pubkey(), pubkey_id(1));
            assert_eq!(event.kind(), 1);
        }
    }

    #[tokio::test]
    async fn query_full_scan_skips_evicted_offsets() {
        // Small buffer: only the last few events survive eviction.
        let probe = build_parsed_worker_message(&event_id(0), &pubkey_id(0), 1, 1, &[]);
        let per_event = 4 + probe.len();
        let capacity_events = 5usize;
        let db = new_test_db(per_event * capacity_events + probe.len() / 2);
        db.initialize().await.unwrap();

        for i in 0..10u32 {
            let bytes =
                build_parsed_worker_message(&event_id(i as usize), &pubkey_id(0), 1, i + 1, &[]);
            db.add_worker_message_bytes(&bytes).await.unwrap();
        }

        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![1]);

        let result = db.query_events_with_filter(filter).unwrap();

        // Indexes still hold all 10 ids, but only the live tail may be returned.
        assert_eq!(result.total_found, capacity_events);
        assert_eq!(result.events.len(), capacity_events);
        assert_eq!(result_created_ats(&result), vec![10, 9, 8, 7, 6]);
    }

    #[tokio::test]
    async fn duplicate_persist_uses_cheap_liveness_check() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let bytes = build_parsed_worker_message(&event_id(0), &pubkey_id(0), 1, 42, &[]);
        let parsed = {
            let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
            wm.content_as_parsed_event().unwrap()
        };

        db.add_parsed_event_with_bytes(parsed, &bytes)
            .await
            .unwrap();
        db.storage.reset_counters();

        // Re-persisting the same event must skip storage via contains_offset,
        // without copying the existing event bytes (no get_event call).
        db.add_parsed_event_with_bytes(parsed, &bytes)
            .await
            .unwrap();

        assert_eq!(db.storage.get_calls.get(), 0);
        assert!(db.storage.contains_calls.get() >= 1);
        assert_eq!(db.storage().load_events().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn duplicate_persist_restores_after_eviction() {
        let probe = build_parsed_worker_message(&event_id(0), &pubkey_id(0), 1, 1, &[]);
        let per_event = 4 + probe.len();
        // Room for ~3 events.
        let db = new_test_db(per_event * 3 + probe.len() / 2);
        db.initialize().await.unwrap();

        let bytes = build_parsed_worker_message(&event_id(0), &pubkey_id(0), 1, 42, &[]);
        {
            let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
            let parsed = wm.content_as_parsed_event().unwrap();
            db.add_parsed_event_with_bytes(parsed, &bytes)
                .await
                .unwrap();
        }

        // Evict it with filler events
        for i in 0..5u32 {
            let filler =
                build_parsed_worker_message(&event_id(10 + i as usize), &pubkey_id(1), 1, i, &[]);
            db.add_worker_message_bytes(&filler).await.unwrap();
        }

        // The stale indexed offset must not suppress re-storage.
        {
            let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
            let parsed = wm.content_as_parsed_event().unwrap();
            db.add_parsed_event_with_bytes(parsed, &bytes)
                .await
                .unwrap();
        }

        let mut filter = QueryFilter::new();
        filter.ids = Some(vec![event_id(0)]);
        let result = db.query_events_with_filter(filter).unwrap();
        assert_eq!(result.total_found, 1);
        assert_eq!(result_created_ats(&result), vec![42]);
    }

    // --------------------------------------------------------------------
    // NIP-09 deletion tombstones
    // --------------------------------------------------------------------

    fn build_deletion(id: &str, author: &str, created_at: u32, tags: &[&[&str]]) -> Vec<u8> {
        build_parsed_worker_message(id, author, EVENT_DELETION, created_at, tags)
    }

    fn query_kind(db: &NostrDB<CountingStorage>, kind: u16) -> QueryResult {
        let mut filter = QueryFilter::new();
        filter.kinds = Some(vec![kind]);
        db.query_events_with_filter(filter).unwrap()
    }

    #[tokio::test]
    async fn a_tag_deletion_hides_cached_event() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        let address = format!("31923:{}:meetup-1", author);
        let target =
            build_parsed_worker_message(&event_id(1), &author, 31923, 1000, &[&["d", "meetup-1"]]);
        db.add_worker_message_bytes(&target).await.unwrap();

        let deletion = build_deletion(&event_id(2), &author, 2000, &[&["a", &address]]);
        db.add_worker_message_bytes(&deletion).await.unwrap();

        let result = query_kind(&db, 31923);
        assert!(result.events.is_empty(), "deleted event must be filtered");
        assert_eq!(db.deleted_count(), 1);

        // The deletion itself stays queryable.
        assert_eq!(query_kind(&db, EVENT_DELETION).total_found, 1);
    }

    #[tokio::test]
    async fn e_tag_deletion_hides_cached_event() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        let target = build_parsed_worker_message(&event_id(1), &author, 1, 1000, &[]);
        db.add_worker_message_bytes(&target).await.unwrap();

        let deletion = build_deletion(&event_id(2), &author, 2000, &[&["e", &event_id(1)]]);
        db.add_worker_message_bytes(&deletion).await.unwrap();

        assert!(query_kind(&db, 1).events.is_empty());
        assert_eq!(db.deleted_count(), 1);
    }

    #[tokio::test]
    async fn deletion_before_target_arrival() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        // Deletion references an event id the cache has not seen yet.
        let deletion = build_deletion(&event_id(2), &author, 2000, &[&["e", &event_id(1)]]);
        db.add_worker_message_bytes(&deletion).await.unwrap();

        let target = build_parsed_worker_message(&event_id(1), &author, 1, 1000, &[]);
        db.add_worker_message_bytes(&target).await.unwrap();

        assert!(query_kind(&db, 1).events.is_empty());
        assert_eq!(db.deleted_count(), 1);
    }

    #[tokio::test]
    async fn author_mismatch_ignored() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        let other = pubkey_id(2);
        let target = build_parsed_worker_message(&event_id(1), &author, 1, 1000, &[]);
        db.add_worker_message_bytes(&target).await.unwrap();

        // e-tag deletion signed by someone else.
        let bad_e = build_deletion(&event_id(2), &other, 2000, &[&["e", &event_id(1)]]);
        db.add_worker_message_bytes(&bad_e).await.unwrap();
        assert_eq!(query_kind(&db, 1).total_found, 1);
        assert_eq!(db.deleted_count(), 0);

        // a-tag deletion whose address author differs from the signer.
        let address = format!("31923:{}:meetup-1", author);
        let calendar =
            build_parsed_worker_message(&event_id(3), &author, 31923, 1000, &[&["d", "meetup-1"]]);
        db.add_worker_message_bytes(&calendar).await.unwrap();
        let bad_a = build_deletion(&event_id(4), &other, 2000, &[&["a", &address]]);
        db.add_worker_message_bytes(&bad_a).await.unwrap();
        assert_eq!(query_kind(&db, 31923).total_found, 1);
        assert_eq!(db.deleted_count(), 0);
    }

    #[tokio::test]
    async fn a_tag_freshness_guard_keeps_republish() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        let address = format!("31923:{}:meetup-1", author);
        let v1 =
            build_parsed_worker_message(&event_id(1), &author, 31923, 1000, &[&["d", "meetup-1"]]);
        db.add_worker_message_bytes(&v1).await.unwrap();

        let deletion = build_deletion(&event_id(2), &author, 2000, &[&["a", &address]]);
        db.add_worker_message_bytes(&deletion).await.unwrap();

        // Re-published after the deletion: must survive.
        let v2 =
            build_parsed_worker_message(&event_id(3), &author, 31923, 3000, &[&["d", "meetup-1"]]);
        db.add_worker_message_bytes(&v2).await.unwrap();

        let result = query_kind(&db, 31923);
        assert_eq!(result.total_found, 1);
        assert_eq!(result_created_ats(&result), vec![3000]);
        assert_eq!(db.deleted_count(), 1);
    }

    #[tokio::test]
    async fn no_deletions_leaves_queries_untouched() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        for i in 0..3usize {
            let bytes = build_parsed_worker_message(&event_id(i), &author, 1, 1000 + i as u32, &[]);
            db.add_worker_message_bytes(&bytes).await.unwrap();
        }

        let result = query_kind(&db, 1);
        assert_eq!(result.total_found, 3);
        assert_eq!(db.deleted_count(), 0);
    }

    #[tokio::test]
    async fn rebuild_replays_shard_resident_deletions() {
        let db = new_test_db(1024 * 1024);
        db.initialize().await.unwrap();

        let author = pubkey_id(1);
        let target = build_parsed_worker_message(&event_id(1), &author, 1, 1000, &[]);
        db.add_worker_message_bytes(&target).await.unwrap();
        let deletion = build_deletion(&event_id(2), &author, 2000, &[&["e", &event_id(1)]]);
        db.add_worker_message_bytes(&deletion).await.unwrap();
        assert!(query_kind(&db, 1).events.is_empty());

        // Simulates a restart where the kind 5 is still in the shard snapshot:
        // re-indexing must re-apply the deletion (WAL covers the evicted case).
        db.rebuild_indexes_from_storage().unwrap();
        assert!(query_kind(&db, 1).events.is_empty());
        assert_eq!(db.deleted_count(), 1);
    }
}
