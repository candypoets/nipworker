use nipworker_core::traits::{Storage, StorageError};
use nipworker_core::types::nostr::Filter;
use std::collections::HashMap;
use tokio::sync::RwLock;

pub struct InMemoryStorage {
    events: RwLock<HashMap<String, Vec<u8>>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Storage for InMemoryStorage {
    async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        let guard = self.events.read().await;
        Ok(guard.values().cloned().collect())
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        let key = hex::encode(event_bytes);
        let mut guard = self.events.write().await;
        guard.insert(key, event_bytes.to_vec());
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        Ok(())
    }
}
