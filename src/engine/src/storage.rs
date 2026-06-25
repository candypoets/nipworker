use async_trait::async_trait;
use nipworker_core::storage::{NostrDbStorage as CoreNostrDbStorage, PersistentNostrDbStorage};
use nipworker_core::traits::{Storage, StorageError};
use nipworker_core::types::nostr::Filter;

use crate::ring_buffer_persist::IndexedDbBlobStore;

const DEFAULT_RELAYS: &[&str] = &[
    "wss://relay.snort.social",
    "wss://relay.damus.io",
    "wss://relay.primal.net",
];
const INDEXER_RELAYS: &[&str] = &[
    "wss://user.kindpag.es",
    "wss://relay.nos.social",
    "wss://purplepag.es",
    "wss://profiles.nostr1.com",
];

pub struct NostrDbStorage {
    inner: PersistentNostrDbStorage<IndexedDbBlobStore>,
}

impl NostrDbStorage {
    pub fn new(
        max_buffer_size: usize,
        default_relays: Vec<String>,
        indexer_relays: Vec<String>,
    ) -> Self {
        let default_relays = if default_relays.is_empty() {
            DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect()
        } else {
            default_relays
        };
        let indexer_relays = if indexer_relays.is_empty() {
            INDEXER_RELAYS.iter().map(|s| s.to_string()).collect()
        } else {
            indexer_relays
        };
        let core = CoreNostrDbStorage::new(
            "nipworker".to_string(),
            max_buffer_size,
            default_relays,
            indexer_relays,
        );

        Self {
            inner: PersistentNostrDbStorage::new(
                core,
                IndexedDbBlobStore::new("nipworker".to_string()),
            ),
        }
    }
}

#[async_trait(?Send)]
impl Storage for NostrDbStorage {
    async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        self.inner.query(filters).await
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        self.inner.persist(event_bytes).await
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        self.inner.initialize().await
    }
}
