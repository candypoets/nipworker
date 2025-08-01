# NostrDB - Rust Database Implementation

This directory contains a comprehensive Rust implementation of a Nostr event database, converted from the original Go implementation. The database provides in-memory indexing with persistent storage capabilities optimized for Nostr event queries.

## Architecture Overview

The database consists of several key components:

### Core Components

1. **`index.rs`** - Main database implementation (`NostrDB`)
2. **`types.rs`** - Type definitions and data structures
3. **`storage.rs`** - IndexedDB storage backend for persistence
4. **`tests.rs`** - Comprehensive test suite

## Key Features

### In-Memory Indexing
- **Primary Index**: Event ID → Event mapping
- **Secondary Indexes**: 
  - Kind-based indexing for event types
  - Author-based indexing by pubkey
  - Tag-based indexing (e, p, a, d tags)
  - Special profile index for kind 0 events

### Query Capabilities
- Filter by event IDs, authors, kinds
- Tag-based filtering (e-tags, p-tags, a-tags, d-tags)
- Time range filtering (since/until)
- Full-text search in event content
- Limit and pagination support
- Complex multi-filter queries with efficient intersection

### Persistent Storage
- IndexedDB backend for web environments
- Batch operations for performance
- Configurable storage limits
- Reset and refill capabilities

### Performance Optimizations
- Pre-allocated data structures based on frequency analysis
- Efficient set operations for query intersection
- Memory usage estimation
- Concurrent access with RwLock

## Data Types

### ProcessedNostrEvent
Extended Nostr event with extracted tags for efficient filtering:
```rust
pub struct ProcessedNostrEvent {
    pub event: Event,           // Original Nostr event
    pub e_tags: Vec<String>,    // Extracted 'e' tags
    pub p_tags: Vec<String>,    // Extracted 'p' tags  
    pub a_tags: Vec<String>,    // Extracted 'a' tags
    pub d_tags: Vec<String>,    // Extracted 'd' tags
    pub parsed: Option<Value>,  // Parsed content
    pub requests: Option<Vec<Request>>, // Associated requests
    pub relays: Vec<String>,    // Relay sources
}
```

### DatabaseIndexes
In-memory indexes for fast querying:
```rust
pub struct DatabaseIndexes {
    pub events_by_id: HashMap<String, ProcessedNostrEvent>,
    pub events_by_kind: HashMap<u64, HashSet<String>>,
    pub events_by_pubkey: HashMap<String, HashSet<String>>,
    pub events_by_e_tag: HashMap<String, HashSet<String>>,
    pub events_by_p_tag: HashMap<String, HashSet<String>>,
    pub events_by_a_tag: HashMap<String, HashSet<String>>,
    pub events_by_d_tag: HashMap<String, HashSet<String>>,
    pub profiles_by_pubkey: HashMap<String, ProcessedNostrEvent>,
}
```

## Usage Examples

### Basic Operations
```rust
use crate::db::{NostrDB, DatabaseConfig};

// Create and initialize database
let db = NostrDB::new();
db.initialize().await?;

// Add an event
let parsed_event = ParsedEvent::new(nostr_event);
db.add_event(parsed_event).await?;

// Retrieve event by ID
let event = db.get_event(&event_id).await;

// Check if event exists
let exists = db.has_event(&event_id).await;

// Get profile by pubkey
let profile = db.get_profile(&pubkey).await;
```

### Querying
```rust
use nostr::Filter;

// Query by kind
let filter = Filter::new().kind(Kind::TextNote);
let events = db.query_events(filter).await?;

// Query by author
let filter = Filter::new().author(pubkey);
let events = db.query_events(filter).await?;

// Complex query with multiple filters
let filter = Filter::new()
    .kind(Kind::TextNote)
    .author(pubkey)
    .since(timestamp)
    .limit(50);
let events = db.query_events(filter).await?;
```

### Batch Operations
```rust
// Process multiple requests
let (remaining_requests, events) = db
    .query_events_for_requests(requests, cache_only)
    .await?;

// Save events to persistent storage
db.save_events_to_storage(events).await?;
```

## Configuration

```rust
pub struct DatabaseConfig {
    pub max_events_in_storage: usize,  // Default: 25,000
    pub batch_size: usize,             // Default: 1,000
    pub debug_logging: bool,           // Default: false
}
```

## Storage Backend

### IndexedDB Implementation
- Optimized for web environments
- Supports batch operations
- Automatic index creation
- Error handling and recovery
- Configurable batch sizes

### Storage Trait
```rust
#[async_trait]
pub trait EventStorage: Send + Sync {
    async fn save_events(&self, events: Vec<ProcessedNostrEvent>) -> Result<(), DatabaseError>;
    async fn load_events(&self) -> Result<Vec<ProcessedNostrEvent>, DatabaseError>;
    async fn clear_storage(&self) -> Result<(), DatabaseError>;
    async fn get_stats(&self) -> Result<HashMap<String, Value>, DatabaseError>;
}
```

## Error Handling

```rust
pub enum DatabaseError {
    NotInitialized,
    InvalidEventId(String),
    InvalidPubkey(String),
    SerializationError(serde_json::Error),
    StorageError(String),
    FilterError(String),
    ConcurrencyError(String),
}
```

## Performance Characteristics

### Query Performance
- O(1) lookup for event by ID
- O(1) lookup for profiles by pubkey
- O(k) filtering where k is the size of the smallest index
- Efficient set intersections for multi-filter queries

### Memory Usage
- Estimated memory usage tracking
- Pre-allocated data structures
- Configurable storage limits
- Batch processing for large datasets

### Concurrency
- Read-write locks for concurrent access
- Non-blocking reads
- Write operations are serialized
- Thread-safe operations

## Testing

The implementation includes comprehensive tests covering:

- Basic CRUD operations
- Complex query scenarios
- Tag extraction and indexing
- Profile handling
- Error conditions
- Concurrent access patterns
- Storage operations
- Performance characteristics

Run tests with:
```bash
cargo test --lib db::tests
```

## Integration

The database implements the `EventDatabase` trait for seamless integration with the Nostr client:

```rust
#[async_trait]
impl EventDatabase for NostrDB {
    async fn query_events_for_requests(
        &self,
        requests: Vec<Request>,
        cache_only: bool,
    ) -> Result<(Vec<Request>, Vec<ParsedEvent>)>;
    
    async fn query_events(&self, filter: Filter) -> Result<Vec<ParsedEvent>>;
    async fn add_event(&self, event: ParsedEvent) -> Result<()>;
    async fn save_events_to_persistent_storage(&self, events: Vec<ParsedEvent>) -> Result<()>;
}
```

## Migration from Go

This Rust implementation maintains feature parity with the original Go implementation while providing:

- Better memory safety through Rust's ownership system
- Improved error handling with Result types
- More efficient concurrent access patterns
- Type safety for Nostr event handling
- WASM compatibility for web deployment

## Future Enhancements

- [ ] Compression for storage efficiency
- [ ] Advanced query optimization
- [ ] Event validation hooks
- [ ] Metrics and monitoring
- [ ] Custom storage backends
- [ ] Event expiration policies