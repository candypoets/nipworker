use async_lock::RwLock;
use async_trait::async_trait;
use nipworker_core::traits::{Storage, StorageError};
use nipworker_core::types::nostr::Filter;
use std::collections::HashMap;

/// Simple in-memory storage for WASM engine.
/// In production this should be backed by IndexedDB.
#[derive(Debug)]
pub struct MemoryStorage {
    events: RwLock<HashMap<String, Vec<u8>>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            events: RwLock::new(HashMap::new()),
        }
    }
}

#[async_trait(?Send)]
impl Storage for MemoryStorage {
    async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        // Simplified: return all events regardless of filters.
        let guard = self.events.read().await;
        Ok(guard.values().cloned().collect())
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        // Use the hex-encoded bytes as a collision-free key.
        let key = hex::encode(event_bytes);
        let mut guard = self.events.write().await;
        guard.insert(key, event_bytes.to_vec());
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        Ok(())
    }
}
