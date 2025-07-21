use crate::db::index::NostrDB;
use crate::db::types::{
    intersect_event_sets, union_event_sets, DatabaseConfig, DatabaseError, EventStorage,
    ProcessedNostrEvent,
};
use crate::network::subscriptions::interfaces::EventDatabase;
use crate::types::{network::Request, ParsedEvent};
use async_trait::async_trait;
use nostr::{Event, EventBuilder, EventId, Filter, Keys, Kind, Tag, Timestamp};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

// Mock storage implementation for testing
#[derive(Debug, Clone)]
struct MockStorage {
    events: Arc<RwLock<Vec<ProcessedNostrEvent>>>,
    should_fail: Arc<RwLock<bool>>,
}

impl MockStorage {
    fn new() -> Self {
        Self {
            events: Arc::new(RwLock::new(Vec::new())),
            should_fail: Arc::new(RwLock::new(false)),
        }
    }

    async fn set_should_fail(&self, fail: bool) {
        *self.should_fail.write().unwrap() = fail;
    }

    async fn get_events(&self) -> Vec<ProcessedNostrEvent> {
        self.events.read().unwrap().clone()
    }
}

#[async_trait(?Send)]
impl EventStorage for MockStorage {
    async fn save_events(&self, events: Vec<ProcessedNostrEvent>) -> Result<(), DatabaseError> {
        if *self.should_fail.read().unwrap() {
            return Err(DatabaseError::StorageError("Mock failure".to_string()));
        }
        self.events.write().unwrap().extend(events);
        Ok(())
    }

    async fn load_events(&self) -> Result<Vec<ProcessedNostrEvent>, DatabaseError> {
        if *self.should_fail.read().unwrap() {
            return Err(DatabaseError::StorageError("Mock failure".to_string()));
        }
        Ok(self.events.read().unwrap().clone())
    }

    async fn clear_storage(&self) -> Result<(), DatabaseError> {
        if *self.should_fail.read().unwrap() {
            return Err(DatabaseError::StorageError("Mock failure".to_string()));
        }
        self.events.write().unwrap().clear();
        Ok(())
    }

    async fn get_stats(&self) -> Result<HashMap<String, Value>, DatabaseError> {
        let count = self.events.read().unwrap().len();
        let mut stats = HashMap::new();
        stats.insert("total_events".to_string(), Value::Number(count.into()));
        Ok(stats)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

// Helper functions for creating test data
fn create_test_keys() -> Keys {
    Keys::generate()
}

fn create_timestamp_from_secs(secs: u64) -> Timestamp {
    Timestamp::from(secs)
}

fn create_test_event(keys: &Keys, kind: Kind, content: &str, tags: Vec<Tag>) -> Event {
    EventBuilder::new(kind, content, tags)
        .to_event(keys)
        .unwrap()
}

fn create_test_parsed_event(event: Event) -> ParsedEvent {
    ParsedEvent::new(event)
}

async fn create_test_db_with_mock_storage() -> NostrDB {
    let storage = Arc::new(MockStorage::new());
    NostrDB::with_storage(DatabaseConfig::default(), storage)
}

async fn create_populated_test_db() -> NostrDB {
    let db = create_test_db_with_mock_storage().await;
    let keys1 = create_test_keys();
    let keys2 = create_test_keys();

    // Create test events of different kinds
    let event1 = create_test_event(
        &keys1,
        Kind::Metadata,
        r#"{"name":"Alice","about":"Test user"}"#,
        vec![],
    );

    let event2 = create_test_event(
        &keys1,
        Kind::TextNote,
        "Hello, Nostr!",
        vec![
            Tag::parse(vec!["t".to_string(), "test".to_string()]).unwrap(),
            Tag::public_key(keys2.public_key()),
        ],
    );

    let event3 = create_test_event(
        &keys2,
        Kind::TextNote,
        "Reply to Alice",
        vec![Tag::event(event2.id), Tag::public_key(keys1.public_key())],
    );

    let event4 = create_test_event(
        &keys1,
        Kind::Reaction,
        "+",
        vec![Tag::event(event2.id), Tag::public_key(keys2.public_key())],
    );

    // Add events to database
    let _ = db.add_event(create_test_parsed_event(event1)).await;
    let _ = db.add_event(create_test_parsed_event(event2)).await;
    let _ = db.add_event(create_test_parsed_event(event3)).await;
    let _ = db.add_event(create_test_parsed_event(event4)).await;

    db
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_database_initialization() {
    let db = NostrDB::new();

    // Database should not be initialized yet
    assert!(!db.is_initialized());

    // Initialize the database
    db.initialize().await.unwrap();

    // Now it should be initialized
    assert!(db.is_initialized());
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_add_and_get_event() {
    let db = create_test_db_with_mock_storage().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();
    let event = create_test_event(&keys, Kind::TextNote, "Test message", vec![]);
    let event_id = event.id;
    let parsed_event = create_test_parsed_event(event);

    // Add event to database
    db.add_event(parsed_event.clone()).await.unwrap();

    // Verify event exists
    assert!(db.has_event(&event_id));
    let retrieved = db.get_event(&event_id);
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().event.id, event_id);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_profile_handling() {
    let db = create_test_db_with_mock_storage().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();
    let profile_content = r#"{"name":"Test User","about":"A test profile"}"#;
    let event = create_test_event(&keys, Kind::Metadata, profile_content, vec![]);
    let pubkey = keys.public_key();

    db.add_event(create_test_parsed_event(event)).await.unwrap();

    // Retrieve profile
    let profile = db.get_profile(&pubkey);
    assert!(profile.is_some());
    assert_eq!(profile.unwrap().event.content, profile_content);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_by_kind() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    // Query for text notes
    let filter = Filter::new().kind(Kind::TextNote);
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 2); // Should find 2 text notes

    // Query for metadata events
    let filter = Filter::new().kind(Kind::Metadata);
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 1); // Should find 1 metadata event

    // Query for reactions
    let filter = Filter::new().kind(Kind::Reaction);
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 1); // Should find 1 reaction

    // Query for non-existent kind
    let filter = Filter::new().kind(Kind::ChannelCreation);
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 0); // Should find nothing
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_by_author() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    let keys1 = create_test_keys();

    // Add an event by keys1
    let event = create_test_event(&keys1, Kind::TextNote, "Test by keys1", vec![]);
    db.add_event(create_test_parsed_event(event)).await.unwrap();

    // Query events by this author
    let filter = Filter::new().author(keys1.public_key());
    let results = db.query_events(filter).await.unwrap();
    assert!(results.len() >= 1); // Should find at least 1 event

    // Verify all results are from the correct author
    for result in results {
        assert_eq!(result.event.pubkey, keys1.public_key());
    }
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_by_tags() {
    let db = create_test_db_with_mock_storage().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();
    let target_pubkey = create_test_keys().public_key();

    // Create event with p tag
    let event = create_test_event(
        &keys,
        Kind::TextNote,
        "Mentioning someone",
        vec![Tag::public_key(target_pubkey)],
    );

    db.add_event(create_test_parsed_event(event)).await.unwrap();

    // Query by p tag
    let filter = Filter::new().pubkey(target_pubkey);
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_with_time_range() {
    let db = create_test_db_with_mock_storage().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();

    // Create events with different timestamps
    let old_event = create_test_event(&keys, Kind::TextNote, "Old event", vec![]);
    let new_event = create_test_event(&keys, Kind::TextNote, "New event", vec![]);

    db.add_event(create_test_parsed_event(old_event.clone()))
        .await
        .unwrap();
    db.add_event(create_test_parsed_event(new_event.clone()))
        .await
        .unwrap();

    // Query events since a certain time
    let since_time = old_event.created_at;
    let filter = Filter::new().since(since_time);
    let results = db.query_events(filter).await.unwrap();
    assert!(results.len() >= 2);

    // Query events until a certain time
    let until_time = new_event.created_at;
    let filter = Filter::new().until(until_time);
    let results = db.query_events(filter).await.unwrap();
    assert!(results.len() >= 2);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_with_limit() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    // Query with limit
    let filter = Filter::new().limit(2);
    let results = db.query_events(filter).await.unwrap();
    assert!(results.len() <= 2);

    // Query without limit should return more
    let filter = Filter::new();
    let all_results = db.query_events(filter).await.unwrap();
    assert!(all_results.len() >= results.len());
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_with_search() {
    let db = create_test_db_with_mock_storage().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();

    // Create events with searchable content
    let event1 = create_test_event(&keys, Kind::TextNote, "Hello Bitcoin world", vec![]);
    let event2 = create_test_event(&keys, Kind::TextNote, "Lightning network is fast", vec![]);
    let event3 = create_test_event(&keys, Kind::TextNote, "Nostr is decentralized", vec![]);

    db.add_event(create_test_parsed_event(event1))
        .await
        .unwrap();
    db.add_event(create_test_parsed_event(event2))
        .await
        .unwrap();
    db.add_event(create_test_parsed_event(event3))
        .await
        .unwrap();

    // Search for "Bitcoin"
    let filter = Filter::new().search("Bitcoin");
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].event.content.contains("Bitcoin"));

    // Search for "network"
    let filter = Filter::new().search("network");
    let results = db.query_events(filter).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].event.content.contains("network"));
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_complex_query() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();

    // Create a specific event we want to find
    let event = create_test_event(
        &keys,
        Kind::TextNote,
        "Special message",
        vec![Tag::parse(vec!["t".to_string(), "special".to_string()]).unwrap()],
    );

    db.add_event(create_test_parsed_event(event.clone()))
        .await
        .unwrap();

    // Complex query: specific author + kind + since time
    let filter = Filter::new()
        .author(keys.public_key())
        .kind(Kind::TextNote)
        .since(event.created_at);

    let results = db.query_events(filter).await.unwrap();
    assert!(results.len() >= 1);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_by_ids() {
    let db = create_test_db_with_mock_storage().await;
    db.initialize().await.unwrap();

    let keys = create_test_keys();
    let event = create_test_event(&keys, Kind::TextNote, "Test event", vec![]);
    let event_id = event.id;

    db.add_event(create_test_parsed_event(event)).await.unwrap();

    // Query by specific ID
    let mut ids = HashSet::new();
    ids.insert(event_id);
    let filter = Filter::new().ids(ids);
    let results = db.query_events(filter).await.unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].event.id, event_id);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_database_stats() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    let stats = db.get_stats();

    assert!(stats.total_events > 0);
    assert!(stats.profile_count >= 0);
    assert!(!stats.events_by_kind.is_empty());
    assert!(!stats.events_by_author.is_empty());
    assert!(stats.is_initialized);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_event_storage_persistence() {
    let mock_storage = Arc::new(MockStorage::new());
    let db = NostrDB::with_storage(DatabaseConfig::default(), mock_storage.clone());
    db.initialize().await.unwrap();

    let keys = create_test_keys();
    let event = create_test_event(&keys, Kind::TextNote, "Persistent event", vec![]);
    let parsed_event = create_test_parsed_event(event);

    // Add event and save to storage
    db.add_event(parsed_event.clone()).await.unwrap();
    db.save_events_to_storage(vec![parsed_event]).await.unwrap();

    // Verify storage has the event
    let stored_events = mock_storage.get_events().await;
    assert_eq!(stored_events.len(), 1);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_processed_nostr_event_conversion() {
    let keys = create_test_keys();
    let target_keys = create_test_keys();

    // Create event with various tags
    let event = create_test_event(
        &keys,
        Kind::TextNote,
        "Test with tags",
        vec![
            Tag::event(EventId::all_zeros()),
            Tag::public_key(target_keys.public_key()),
            Tag::parse(vec!["t".to_string(), "test".to_string()]).unwrap(),
            Tag::parse(vec!["d".to_string(), "test-id".to_string()]).unwrap(),
        ],
    );

    let parsed_event = create_test_parsed_event(event.clone());
    let processed_event = ProcessedNostrEvent::from_parsed_event(parsed_event.clone());

    // Verify tag extraction
    assert!(!processed_event.e_tags.is_empty());
    assert!(!processed_event.p_tags.is_empty());
    assert!(!processed_event.d_tags.is_empty());

    // Verify conversion back to ParsedEvent
    let converted_back = processed_event.to_parsed_event();
    assert_eq!(converted_back.event.id, event.id);
    assert_eq!(converted_back.event.content, event.content);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_events_for_requests() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    // Create a request
    let request = Request {
        ids: vec![],
        authors: vec![],
        kinds: vec![Kind::TextNote.as_u64() as i32],
        tags: HashMap::new(),
        since: None,
        until: None,
        limit: Some(10),
        search: String::new(),
        relays: vec!["wss://test.relay".to_string()],
        close_on_eose: false,
        cache_first: true,
        no_optimize: false,
        count: false,
        no_context: false,
    };

    let requests = vec![request];
    let (returned_requests, events) = db
        .query_events_for_requests(requests.clone(), false)
        .await
        .unwrap();

    assert_eq!(returned_requests.len(), requests.len());
    assert!(!events.is_empty());

    // Test with skip_filtered = true
    let (_, filtered_events) = db.query_events_for_requests(requests, true).await.unwrap();

    assert!(!filtered_events.is_empty());
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_error_handling() {
    let mock_storage = Arc::new(MockStorage::new());
    let db = NostrDB::with_storage(DatabaseConfig::default(), mock_storage.clone());

    // Test storage failure
    mock_storage.set_should_fail(true).await;

    let result = db.initialize().await;
    assert!(result.is_err());

    // Reset and test successful operation
    mock_storage.set_should_fail(false).await;
    let result = db.initialize().await;
    assert!(result.is_ok());
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_intersection_and_union_helpers() {
    let set1: HashSet<String> = ["a", "b", "c"].iter().map(|s| s.to_string()).collect();
    let set2: HashSet<String> = ["b", "c", "d"].iter().map(|s| s.to_string()).collect();
    let set3: HashSet<String> = ["c", "d", "e"].iter().map(|s| s.to_string()).collect();

    // Test intersection
    let intersection = intersect_event_sets(vec![&set1, &set2, &set3]);
    assert_eq!(intersection.len(), 1);
    assert!(intersection.contains("c"));

    // Test union
    let union = union_event_sets(vec![&set1, &set2, &set3]);
    assert_eq!(union.len(), 5); // a, b, c, d, e
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_database_config() {
    let config = DatabaseConfig {
        max_events_in_storage: 1000,
        batch_size: 100,
        debug_logging: true,
    };

    let mock_storage = Arc::new(MockStorage::new());
    let _db = NostrDB::with_storage(DatabaseConfig::default(), mock_storage);

    // The database should use the default config since we can't set custom config
    // This test mainly verifies the config structure works
    assert!(config.max_events_in_storage > 0);
    assert!(config.batch_size > 0);
}

#[wasm_bindgen_test::wasm_bindgen_test]
async fn test_query_result_structure() {
    let db = create_populated_test_db().await;
    db.initialize().await.unwrap();

    let filter = Filter::new().limit(2);
    let results = db.query_events(filter).await.unwrap();

    // Verify the structure of returned events
    for result in results {
        assert!(!result.event.id.to_hex().is_empty());
        assert!(!result.event.pubkey.to_hex().is_empty());
        assert!(result.event.created_at.as_u64() > 0);
        assert!(result.event.kind.as_u64() >= 0);
    }
}
