use rustc_hash::{FxHashMap, FxHashSet};
use std::cell::RefCell;
use std::rc::Rc;

/// Database configuration
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// Maximum number of events to keep in IndexedDB
    pub max_events_in_storage: usize,
    /// Batch size for database operations
    pub batch_size: usize,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            max_events_in_storage: 25_000,
            batch_size: 1000,
        }
    }
}

/// Statistics about the database
#[derive(Debug, Clone, Default)]
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
#[allow(non_snake_case)]
pub struct QueryFilter {
    pub ids: Option<Vec<String>>,     // was Vec<EventId>
    pub authors: Option<Vec<String>>, // was Vec<PublicKey>
    pub kinds: Option<Vec<u16>>,      // keep u16 (fb uses ushort)
    pub e_tags: Option<Vec<String>>,  // lowercase e (NIP-10)
    pub E_tags: Option<Vec<String>>,  // uppercase E (NIP-22)
    pub p_tags: Option<Vec<String>>,  // lowercase p (NIP-10)
    pub P_tags: Option<Vec<String>>,  // uppercase P (NIP-22)
    pub a_tags: Option<Vec<String>>,
    pub d_tags: Option<Vec<String>>,
    pub q_tags: Option<Vec<String>>, // q tag (quote/citation)
    pub since: Option<u32>,          // use u32 (fb since/until are i32)
    pub until: Option<u32>,
    pub limit: Option<usize>,
    pub search: Option<String>,
}

impl QueryFilter {
    pub fn new() -> Self {
        Self {
            ids: None,
            authors: None,
            kinds: None,
            e_tags: None,
            E_tags: None,
            p_tags: None,
            P_tags: None,
            a_tags: None,
            d_tags: None,
            q_tags: None,
            since: None,
            until: None,
            limit: None,
            search: None,
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

    /// Cheap liveness check: does `event_offset` still point to a complete,
    /// readable event? Must NOT copy event bytes (unlike `get_event`).
    fn contains_offset(&self, event_offset: u64) -> bool;

    /// Load all events from persistent storage
    fn load_events(&self) -> Result<Vec<u64>, DatabaseError>;

    /// Clear all events from persistent storage
    async fn clear_storage(&self) -> Result<(), DatabaseError>;

    /// Downcast to Any for testing purposes
    fn as_any(&self) -> &dyn std::any::Any;
}

pub type EventKey = u32;

#[derive(Debug, Clone, Copy)]
pub struct EventRecord {
    pub key: EventKey,
    pub offset: u64,
    /// Event creation time captured at index time, so since/until pruning and
    /// result sorting never need to read event bytes from storage.
    pub created_at: u32,
}

/// Index types for efficient querying (concurrent)
pub type EventIdIndex = Rc<RefCell<FxHashMap<String, EventRecord>>>;
pub type EventKeyIndex = Rc<RefCell<FxHashMap<EventKey, EventRecord>>>;
pub type KindIndex = Rc<RefCell<FxHashMap<u16, FxHashSet<EventKey>>>>;
pub type PubkeyIndex = Rc<RefCell<FxHashMap<String, FxHashSet<EventKey>>>>;
pub type TagIndex = Rc<RefCell<FxHashMap<String, FxHashSet<EventKey>>>>;

/// Database indexes for fast querying (concurrent)
#[allow(non_snake_case)]
pub struct DatabaseIndexes {
    /// Primary index: event_id -> event
    pub events_by_id: EventIdIndex,
    /// Internal query index: compact event key -> storage offset
    pub events_by_key: EventKeyIndex,
    next_event_key: Rc<RefCell<EventKey>>,
    /// Secondary indexes
    pub events_by_kind: KindIndex,
    pub events_by_pubkey: PubkeyIndex,
    pub events_by_e_tag: TagIndex, // lowercase e (NIP-10)
    pub events_by_E_tag: TagIndex, // uppercase E (NIP-22)
    pub events_by_p_tag: TagIndex, // lowercase p (NIP-10)
    pub events_by_P_tag: TagIndex, // uppercase P (NIP-22)
    pub events_by_a_tag: TagIndex,
    pub events_by_d_tag: TagIndex,
    pub events_by_q_tag: TagIndex, // q tag (quote/citation)
}

impl DatabaseIndexes {
    /// Create new empty indexes
    pub fn new() -> Self {
        Self {
            events_by_id: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_key: Rc::new(RefCell::new(FxHashMap::default())),
            next_event_key: Rc::new(RefCell::new(0)),
            events_by_kind: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_pubkey: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_e_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_E_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_p_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_P_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_a_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_d_tag: Rc::new(RefCell::new(FxHashMap::default())),
            events_by_q_tag: Rc::new(RefCell::new(FxHashMap::default())),
        }
    }

    /// Clear all indexes
    pub fn clear(&self) {
        self.events_by_id.borrow_mut().clear();
        self.events_by_key.borrow_mut().clear();
        *self.next_event_key.borrow_mut() = 0;
        self.events_by_kind.borrow_mut().clear();
        self.events_by_pubkey.borrow_mut().clear();
        self.events_by_e_tag.borrow_mut().clear();
        self.events_by_E_tag.borrow_mut().clear();
        self.events_by_p_tag.borrow_mut().clear();
        self.events_by_P_tag.borrow_mut().clear();
        self.events_by_a_tag.borrow_mut().clear();
        self.events_by_d_tag.borrow_mut().clear();
        self.events_by_q_tag.borrow_mut().clear();
    }

    /// Get total number of events
    pub fn event_count(&self) -> usize {
        self.events_by_id.borrow().len()
    }

    pub fn upsert_event_record(&self, event_id: &str, offset: u64, created_at: u32) -> EventKey {
        let mut events_by_id = self.events_by_id.borrow_mut();
        if let Some(record) = events_by_id.get_mut(event_id) {
            record.offset = offset;
            record.created_at = created_at;
            let record = *record;
            self.events_by_key.borrow_mut().insert(record.key, record);
            return record.key;
        }

        let mut next_event_key = self.next_event_key.borrow_mut();
        let key = *next_event_key;
        *next_event_key = next_event_key
            .checked_add(1)
            .expect("event key space exhausted");

        let record = EventRecord {
            key,
            offset,
            created_at,
        };
        events_by_id.insert(event_id.to_string(), record);
        self.events_by_key.borrow_mut().insert(key, record);
        key
    }
}
