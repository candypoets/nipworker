use crate::storage::db::sharded_storage::{ShardId, ShardedRingBufferStorage};
use crate::storage::NostrDbStorage;
use crate::traits::{Storage, StorageError};
use crate::types::nostr::Filter;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

const SYNC_INTERVAL_MS: u64 = 30_000;

#[async_trait(?Send)]
pub trait BlobStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError>;
}

pub struct PersistentNostrDbStorage<B> {
    core: NostrDbStorage,
    blob_store: B,
    last_sync_ms: Mutex<u64>,
}

impl<B> PersistentNostrDbStorage<B> {
    pub fn new(core: NostrDbStorage, blob_store: B) -> Self {
        Self {
            core,
            blob_store,
            last_sync_ms: Mutex::new(0),
        }
    }

    pub fn core(&self) -> &NostrDbStorage {
        &self.core
    }

    fn shard_ids(_storage: &ShardedRingBufferStorage) -> [ShardId; 5] {
        [
            ShardId::Default,
            ShardId::Kind0,
            ShardId::Kind4,
            ShardId::Kind7375,
            ShardId::Kind10002,
        ]
    }

    fn shard_key(shard_id: ShardId) -> &'static str {
        match shard_id {
            ShardId::Default => "shard:default",
            ShardId::Kind0 => "shard:kind0",
            ShardId::Kind4 => "shard:kind4",
            ShardId::Kind7375 => "shard:kind7375",
            ShardId::Kind10002 => "shard:kind10002",
        }
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    fn should_sync(&self) -> bool {
        let now = Self::now_ms();
        let last = self.last_sync_ms.lock().map(|v| *v).unwrap_or(0);
        now.saturating_sub(last) > SYNC_INTERVAL_MS
    }

    fn mark_synced(&self) {
        if let Ok(mut last) = self.last_sync_ms.lock() {
            *last = Self::now_ms();
        }
    }
}

impl<B: BlobStore> PersistentNostrDbStorage<B> {
    async fn hydrate_from_blob_store(&self) -> Result<(), StorageError> {
        let sharded = self.core.sharded_storage();
        let mut shard_bytes = HashMap::new();

        for shard_id in Self::shard_ids(sharded) {
            if let Some(bytes) = self.blob_store.get(Self::shard_key(shard_id)).await? {
                if !bytes.is_empty() {
                    shard_bytes.insert(shard_id, bytes);
                }
            }
        }

        if !shard_bytes.is_empty() {
            sharded.load_all_shards(&shard_bytes).map_err(|e| {
                StorageError::Other(format!("Failed to load persisted shards: {}", e))
            })?;
            self.core.rebuild_indexes_from_storage().map_err(|e| {
                StorageError::Other(format!("Failed to index persisted shards: {}", e))
            })?;
        }

        Ok(())
    }

    async fn sync_to_blob_store(&self) -> Result<(), StorageError> {
        let sharded = self.core.sharded_storage();
        let shard_bytes = sharded.save_all_shards();

        for (shard_id, bytes) in shard_bytes {
            self.blob_store
                .put(Self::shard_key(shard_id), &bytes)
                .await?;
        }

        Ok(())
    }
}

#[async_trait(?Send)]
impl<B: BlobStore> Storage for PersistentNostrDbStorage<B> {
    async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        self.core.query(filters).await
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        self.core.persist(event_bytes).await?;
        if self.should_sync() {
            self.sync_to_blob_store().await?;
            self.mark_synced();
        }
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        self.core.initialize().await?;
        self.hydrate_from_blob_store().await
    }
}
