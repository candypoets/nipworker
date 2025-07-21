use crate::types::{network::Request, ParsedEvent};
use nostr::{Event, EventId, Kind, PublicKey, SingleLetterTag, Timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Represents a Nostr event with extracted tags for easier filtering
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessedNostrEvent {
    /// The original Nostr event
    #[serde(flatten)]
    pub event: Event,
    /// Extracted 'e' tags (event references)
    pub e_tags: Vec<String>,
    /// Extracted 'a' tags (replaceable event references)
    pub a_tags: Vec<String>,
    /// Extracted 'p' tags (pubkey references)
    pub p_tags: Vec<String>,
    /// Extracted 'd' tags (identifier tags)
    pub d_tags: Vec<String>,
    /// Parsed content (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,
    /// Associated requests
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<Request>>,
    /// Relay sources
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relays: Option<Vec<String>>,
}

impl ProcessedNostrEvent {
    /// Create a ProcessedNostrEvent from a ParsedEvent
    pub fn from_parsed_event(event: ParsedEvent) -> Self {
        let mut e_tags = Vec::new();
        let mut p_tags = Vec::new();
        let mut a_tags = Vec::new();
        let mut d_tags = Vec::new();

        // Extract tags by type
        for tag in &event.event.tags {
            if tag.as_vec().len() >= 2 {
                match tag.as_vec()[0].as_str() {
                    "e" => e_tags.push(tag.as_vec()[1].clone()),
                    "p" => p_tags.push(tag.as_vec()[1].clone()),
                    "a" => a_tags.push(tag.as_vec()[1].clone()),
                    "d" => d_tags.push(tag.as_vec()[1].clone()),
                    _ => {}
                }
            }
        }

        Self {
            event: event.event,
            e_tags,
            p_tags,
            a_tags,
            d_tags,
            parsed: event.parsed,
            requests: event.requests,
            relays: Some(event.relays),
        }
    }

    /// Convert back to ParsedEvent
    pub fn to_parsed_event(&self) -> ParsedEvent {
        ParsedEvent {
            event: self.event.clone(),
            parsed: self.parsed.clone(),
            requests: self.requests.clone(),
            relays: self.relays.clone().unwrap_or_default(),
        }
    }

    /// Get the event ID as hex string
    pub fn id(&self) -> String {
        self.event.id.to_hex()
    }

    /// Get the pubkey as hex string
    pub fn pubkey(&self) -> String {
        self.event.pubkey.to_hex()
    }

    /// Get the kind as u64
    pub fn kind(&self) -> u64 {
        self.event.kind.as_u64()
    }

    /// Get the created_at timestamp
    pub fn created_at(&self) -> Timestamp {
        self.event.created_at
    }
}

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
    pub events_by_kind: HashMap<u64, usize>,
    /// Number of events by author
    pub events_by_author: HashMap<String, usize>,
    /// Number of profiles
    pub profile_count: usize,
    /// Memory usage estimate in bytes
    pub estimated_memory_usage: usize,
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
    pub events: Vec<ParsedEvent>,
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
#[async_trait::async_trait(?Send)]
pub trait EventStorage: Send + Sync {
    /// Save events to persistent storage
    async fn save_events(&self, events: Vec<ProcessedNostrEvent>) -> Result<(), DatabaseError>;

    /// Load all events from persistent storage
    async fn load_events(&self) -> Result<Vec<ProcessedNostrEvent>, DatabaseError>;

    /// Clear all events from persistent storage
    async fn clear_storage(&self) -> Result<(), DatabaseError>;

    /// Get storage statistics
    async fn get_stats(&self) -> Result<HashMap<String, serde_json::Value>, DatabaseError>;

    /// Downcast to Any for testing purposes
    fn as_any(&self) -> &dyn std::any::Any;
}

/// Index types for efficient querying
pub type EventIdIndex = HashMap<String, ProcessedNostrEvent>;
pub type KindIndex = HashMap<u64, HashSet<String>>;
pub type PubkeyIndex = HashMap<String, HashSet<String>>;
pub type TagIndex = HashMap<String, HashSet<String>>;
pub type ProfileIndex = HashMap<String, ProcessedNostrEvent>;
pub type RelayIndex = HashMap<String, ProcessedNostrEvent>;

/// Database indexes for fast querying
#[derive(Debug, Default)]
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
    /// Special index for profiles (kind 0 events)
    pub profiles_by_pubkey: ProfileIndex,
    /// Special index for relay lists (kind 10002 events)
    pub relays_by_pubkey: RelayIndex,
}

impl DatabaseIndexes {
    /// Create new empty indexes
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all indexes
    pub fn clear(&mut self) {
        self.events_by_id.clear();
        self.events_by_kind.clear();
        self.events_by_pubkey.clear();
        self.events_by_e_tag.clear();
        self.events_by_p_tag.clear();
        self.events_by_a_tag.clear();
        self.events_by_d_tag.clear();
        self.profiles_by_pubkey.clear();
        self.relays_by_pubkey.clear();
    }

    /// Get total number of events
    pub fn event_count(&self) -> usize {
        self.events_by_id.len()
    }

    /// Get number of profiles
    pub fn profile_count(&self) -> usize {
        self.profiles_by_pubkey.len()
    }

    /// Get statistics about the indexes
    pub fn get_stats(&self) -> DatabaseStats {
        let mut stats = DatabaseStats {
            total_events: self.event_count(),
            profile_count: self.profile_count(),
            is_initialized: true,
            ..Default::default()
        };

        // Count events by kind
        for (kind, event_ids) in &self.events_by_kind {
            stats.events_by_kind.insert(*kind, event_ids.len());
        }

        // Count events by author
        for (pubkey, event_ids) in &self.events_by_pubkey {
            stats
                .events_by_author
                .insert(pubkey.clone(), event_ids.len());
        }

        // Estimate memory usage (rough approximation)
        stats.estimated_memory_usage = self.estimate_memory_usage();

        stats
    }

    /// Estimate memory usage in bytes (rough approximation)
    pub fn estimate_memory_usage(&self) -> usize {
        let mut total = 0;

        // Events by ID (main storage)
        for (id, event) in &self.events_by_id {
            total += id.len() * 2; // String storage
            total += std::mem::size_of::<ProcessedNostrEvent>();
            total += event.event.content.len() * 2; // Content storage
            total += event.e_tags.len() * 64; // Tag storage estimate
            total += event.p_tags.len() * 64;
            total += event.a_tags.len() * 64;
            total += event.d_tags.len() * 64;
        }

        // Index overhead
        total += self.events_by_kind.len() * 64;
        total += self.events_by_pubkey.len() * 64;
        total += self.events_by_e_tag.len() * 64;
        total += self.events_by_p_tag.len() * 64;
        total += self.events_by_a_tag.len() * 64;
        total += self.events_by_d_tag.len() * 64;
        total += self.profiles_by_pubkey.len() * 64;

        total
    }
}

/// Intersection helper for HashSets
pub fn intersect_event_sets(sets: Vec<&HashSet<String>>) -> HashSet<String> {
    if sets.is_empty() {
        return HashSet::new();
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
pub fn union_event_sets(sets: Vec<&HashSet<String>>) -> HashSet<String> {
    let mut result = HashSet::new();
    for set in sets {
        result.extend(set.iter().cloned());
    }
    result
}
