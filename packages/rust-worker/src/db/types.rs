use crate::generated::nostr::fb;
use crate::parsed_event::ParsedEvent;
use nostr::{EventId, Kind, PublicKey, SingleLetterTag, Timestamp};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::rc::Rc;

/// Database configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseConfig {
    /// Maximum number of events to keep in IndexedDB
    pub max_events_in_storage: usize,
    /// Batch size for database operations
    pub batch_size: usize,
    /// Whether to enable debug logging
    pub debug_logging: bool,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            max_events_in_storage: 25_000,
            batch_size: 1000,
            debug_logging: false,
        }
    }
}

/// Statistics about the database
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DatabaseStats {
    /// Total number of events in memory
    pub total_events: usize,
    /// Number of events by kind
    pub events_by_kind: FxHashMap<u64, usize>,
    /// Number of events by author
    pub events_by_author: FxHashMap<String, usize>,
    /// Whether the database is initialized
    pub is_initialized: bool,
}

/// Query filter for internal use
#[derive(Debug, Clone)]
pub struct QueryFilter {
    pub ids: Option<Vec<EventId>>,
    pub authors: Option<Vec<PublicKey>>,
    pub kinds: Option<Vec<Kind>>,
    pub e_tags: Option<Vec<String>>,
    pub p_tags: Option<Vec<String>>,
    pub a_tags: Option<Vec<String>>,
    pub d_tags: Option<Vec<String>>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub limit: Option<usize>,
    pub search: Option<String>,
}

impl QueryFilter {
    /// Create a new empty filter
    pub fn new() -> Self {
        Self {
            ids: None,
            authors: None,
            kinds: None,
            e_tags: None,
            p_tags: None,
            a_tags: None,
            d_tags: None,
            since: None,
            until: None,
            limit: None,
            search: None,
        }
    }

    /// Convert from nostr::Filter
    pub fn from_nostr_filter(filter: &nostr::Filter) -> Self {
        // Convert HashSets to Vecs
        let ids = filter.ids.as_ref().map(|set| set.iter().cloned().collect());
        let authors = filter
            .authors
            .as_ref()
            .map(|set| set.iter().cloned().collect());
        let kinds = filter
            .kinds
            .as_ref()
            .map(|set| set.iter().cloned().collect());

        // Handle generic tags properly
        let e_tag_key = SingleLetterTag::lowercase(nostr::Alphabet::E);
        let p_tag_key = SingleLetterTag::lowercase(nostr::Alphabet::P);
        let a_tag_key = SingleLetterTag::lowercase(nostr::Alphabet::A);
        let d_tag_key = SingleLetterTag::lowercase(nostr::Alphabet::D);

        let e_tags = filter
            .generic_tags
            .get(&e_tag_key)
            .map(|set| set.iter().map(|v| v.to_string()).collect());
        let p_tags = filter
            .generic_tags
            .get(&p_tag_key)
            .map(|set| set.iter().map(|v| v.to_string()).collect());
        let a_tags = filter
            .generic_tags
            .get(&a_tag_key)
            .map(|set| set.iter().map(|v| v.to_string()).collect());
        let d_tags = filter
            .generic_tags
            .get(&d_tag_key)
            .map(|set| set.iter().map(|v| v.to_string()).collect());

        Self {
            ids,
            authors,
            kinds,
            e_tags,
            p_tags,
            a_tags,
            d_tags,
            since: filter.since,
            until: filter.until,
            limit: filter.limit,
            search: filter.search.clone(),
        }
    }
}

/// Result of a database query operation
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Events that matched the query
    pub events: Vec<Vec<u8>>,
    /// Total number of events found (before limit)
    pub total_found: usize,
    /// Whether more events are available
    pub has_more: bool,
    /// Time taken for the query in milliseconds
    pub query_time_ms: u64,
}

/// Error types for database operations
#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("Database not initialized")]
    NotInitialized,
    #[error("Invalid event ID: {0}")]
    InvalidEventId(String),
    #[error("Invalid pubkey: {0}")]
    InvalidPubkey(String),
    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    #[error("Storage error: {0}")]
    StorageError(String),
    #[error("Filter error: {0}")]
    FilterError(String),
    #[error("Concurrency error: {0}")]
    ConcurrencyError(String),
    #[error("Lock error: failed to acquire database lock")]
    LockError,
}

/// Event storage backend trait
pub trait EventStorage {
    async fn initialize_storage(&self) -> Result<(), DatabaseError>;
    async fn add_event_data(&self, event_data: &[u8]) -> Result<u64, DatabaseError>;

    fn get_event(&self, event_offset: u64) -> Result<Option<Vec<u8>>, DatabaseError>;

    /// Load all events from persistent storage
    fn load_events(&self) -> Result<Vec<u64>, DatabaseError>;

    /// Clear all events from persistent storage
    async fn clear_storage(&self) -> Result<(), DatabaseError>;

    /// Downcast to Any for testing purposes
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Index types for efficient querying (concurrent)
pub type EventIdIndex = Rc<RefCell<FxHashMap<String, u64>>>;
pub type KindIndex = Rc<RefCell<FxHashMap<u16, FxHashSet<String>>>>;
pub type PubkeyIndex = Rc<RefCell<FxHashMap<String, FxHashSet<String>>>>;
pub type TagIndex = Rc<RefCell<FxHashMap<String, FxHashSet<String>>>>;
// pub type ProfileIndex = Rc<RefCell<FxHashMap<String, ProcessedNostrEvent>>>;
// pub type RelayIndex = Rc<RefCell<FxHashMap<String, ProcessedNostrEvent>>>;

/// Database indexes for fast querying (concurrent)
pub struct DatabaseIndexes {
    /// Primary index: event_id -> event
    pub events_by_id: EventIdIndex,
    /// Secondary indexes
    pub events_by_kind: KindIndex,
    pub events_by_pubkey: PubkeyIndex,
    pub events_by_e_tag: TagIndex,
    pub events_by_p_tag: TagIndex,
    pub events_by_a_tag: TagIndex,
    pub events_by_d_tag: TagIndex,
    // pub profiles_by_pubkey: ProfileIndex,
    // pub relays_by_pubkey: RelayIndex,
}

impl DatabaseIndexes {
    /// Create new empty indexes
    pub fn new() -> Self {
        Self {
            events_by_id: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_kind: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_pubkey: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_e_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_p_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_a_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_d_tag: Rc::new(RefCell::new(FxHashMap::default())),
            // profiles_by_pubkey: Rc::new(RefCell::new(FxHashMap::default())),
            // relays_by_pubkey: Rc::new(RefCell::new(FxHashMap::default())),
        }
    }

    /// Clear all indexes
    pub fn clear(&self) {
        self.events_by_id.borrow_mut().clear();
        self.events_by_kind.borrow_mut().clear();
        self.events_by_pubkey.borrow_mut().clear();
        self.events_by_e_tag.borrow_mut().clear();
        self.events_by_p_tag.borrow_mut().clear();
        self.events_by_a_tag.borrow_mut().clear();
        self.events_by_d_tag.borrow_mut().clear();
        // self.profiles_by_pubkey.borrow_mut().clear();
        // self.relays_by_pubkey.borrow_mut().clear();
    }

    /// Get total number of events
    pub fn event_count(&self) -> usize {
        self.events_by_id.borrow().len()
    }
}

/// Intersection helper for HashSets
pub fn intersect_event_sets(sets: Vec<&FxHashSet<String>>) -> FxHashSet<String> {
    if sets.is_empty() {
        return FxHashSet::default();
    }

    if sets.len() == 1 {
        return sets[0].clone();
    }

    let mut result = sets[0].clone();
    for set in sets.iter().skip(1) {
        result = result.intersection(set).cloned().collect();
        if result.is_empty() {
            break;
        }
    }

    result
}

/// Union helper for HashSets
pub fn union_event_sets(sets: Vec<&FxHashSet<String>>) -> FxHashSet<String> {
    let mut result = FxHashSet::default();
    for set in sets {
        result.extend(set.iter().cloned());
    }
    result
}
