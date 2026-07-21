use crate::generated::nostr::fb::Request;
use crate::platform::now_millis;
use crate::storage::db::sharded_storage::ShardId;
use crate::storage::NostrDbStorage;
use crate::traits::{Storage, StorageError};
use crate::types::nostr::{Filter, EVENT_DELETION};
use async_trait::async_trait;
use rustc_hash::FxHashSet;
use std::collections::HashMap;
use std::sync::Mutex;
use tracing::{info, warn};

const SYNC_INTERVAL_MS: u64 = 30_000;

/// Blob key for the NIP-09 deletion WAL: raw kind-5 WorkerMessage bytes,
/// appended eagerly on ingest (the 30s shard sync is a loss window).
const TOMBSTONES_KEY: &str = "tombstones";
/// Maximum entries kept in the deletion WAL before oldest-first compaction.
const MAX_WAL_ENTRIES: usize = 8192;

#[async_trait(?Send)]
pub trait BlobStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError>;
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError>;
}

pub struct PersistentNostrDbStorage<B> {
    core: NostrDbStorage,
    blob_store: B,
    last_sync_ms: Mutex<u64>,
    /// Deletion WAL: concatenated `[u32 LE len][WorkerMessage bytes]` entries.
    wal: Mutex<Vec<u8>>,
    /// Ids of deletion events already in the WAL (persist sees duplicates).
    wal_ids: Mutex<FxHashSet<String>>,
    /// Number of entries currently in `wal` (drives compaction).
    wal_entries: Mutex<usize>,
}

impl<B> PersistentNostrDbStorage<B> {
    pub fn new(core: NostrDbStorage, blob_store: B) -> Self {
        Self {
            core,
            blob_store,
            last_sync_ms: Mutex::new(0),
            wal: Mutex::new(Vec::new()),
            wal_ids: Mutex::new(FxHashSet::default()),
            wal_entries: Mutex::new(0),
        }
    }

    pub fn core(&self) -> &NostrDbStorage {
        &self.core
    }

    fn now_ms() -> u64 {
        now_millis()
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

        for &shard_id in ShardId::persistent_ids() {
            let key = shard_id
                .persistence_key()
                .expect("persistent shard must have a blob key");
            let mut stored = self.blob_store.get(key).await?;

            // v0.97 and earlier persisted kind 10002 separately. Fold that
            // data into the replacement shard on first startup after upgrade.
            if stored.is_none() && shard_id == ShardId::Replaceable {
                stored = self.blob_store.get("shard:kind10002").await?;
            }

            if let Some(bytes) = stored {
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
            let key = shard_id
                .persistence_key()
                .expect("snapshot must only contain persistent shards");
            self.blob_store.put(key, &bytes).await?;
        }

        Ok(())
    }

    /// Extract the event id when `bytes` holds a kind-5 (NIP-09) WorkerMessage.
    fn deletion_event_id(bytes: &[u8]) -> Option<String> {
        use crate::generated::nostr::fb::{self, WorkerMessage};
        let wm = flatbuffers::root::<WorkerMessage>(bytes).ok()?;
        match wm.content_type() {
            fb::Message::ParsedEvent => {
                let parsed = wm.content_as_parsed_event()?;
                (parsed.kind() == EVENT_DELETION).then(|| parsed.id().to_string())
            }
            fb::Message::NostrEvent => {
                let event = wm.content_as_nostr_event()?;
                (event.kind() == EVENT_DELETION).then(|| event.id().to_string())
            }
            _ => None,
        }
    }

    /// Keep the newest MAX_WAL_ENTRIES entries, rebuilding the id set to match.
    fn compact_wal(wal: &mut Vec<u8>, entries: &mut usize, wal_ids: &Mutex<FxHashSet<String>>) {
        let mut boundaries = Vec::with_capacity(*entries + 1);
        let mut offset = 0usize;
        while offset + 4 <= wal.len() {
            boundaries.push(offset);
            let len = u32::from_le_bytes(wal[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4 + len;
        }
        if boundaries.len() <= MAX_WAL_ENTRIES {
            return;
        }
        let keep_from = boundaries[boundaries.len() - MAX_WAL_ENTRIES];
        wal.drain(..keep_from);
        *entries = MAX_WAL_ENTRIES;

        let mut ids = FxHashSet::default();
        let mut offset = 0usize;
        while offset + 4 <= wal.len() {
            let len = u32::from_le_bytes(wal[offset..offset + 4].try_into().unwrap()) as usize;
            if let Some(id) = Self::deletion_event_id(&wal[offset + 4..offset + 4 + len]) {
                ids.insert(id);
            }
            offset += 4 + len;
        }
        *wal_ids.lock().unwrap_or_else(|p| p.into_inner()) = ids;
    }

    /// Load the deletion WAL and replay it through the same ingest path used
    /// for live kind-5 events. Must run AFTER the shard rebuild so referenced
    /// events are indexed and can be resolved to keys. Replaying raw events
    /// (rather than derived records) keeps deletion semantics in one place.
    async fn load_tombstones(&self) -> Result<(), StorageError> {
        let Some(bytes) = self.blob_store.get(TOMBSTONES_KEY).await? else {
            return Ok(());
        };

        let mut valid_len = 0usize;
        let mut entries = 0usize;
        {
            let mut wal_ids = self.wal_ids.lock().unwrap_or_else(|p| p.into_inner());
            while valid_len + 4 <= bytes.len() {
                let len = u32::from_le_bytes(bytes[valid_len..valid_len + 4].try_into().unwrap())
                    as usize;
                let start = valid_len + 4;
                let end = start + len;
                if end > bytes.len() {
                    warn!(
                        "[NostrDB] Truncated deletion WAL tail ({} bytes), ignoring",
                        bytes.len() - valid_len
                    );
                    break;
                }
                if let Some(id) = self.core.apply_deletions_from_bytes(&bytes[start..end]) {
                    wal_ids.insert(id);
                }
                valid_len = end;
                entries += 1;
            }
        }
        if entries > 0 {
            info!(
                "[NostrDB] Replayed {} deletion(s) from tombstone WAL",
                entries
            );
        }

        // Keep only the valid prefix for future appends.
        let mut wal = self.wal.lock().unwrap_or_else(|p| p.into_inner());
        wal.clear();
        wal.extend_from_slice(&bytes[..valid_len]);
        *self.wal_entries.lock().unwrap_or_else(|p| p.into_inner()) = entries;
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

        // NIP-09: append deletions to the tombstone WAL and flush eagerly —
        // waiting for the 30s shard sync would be a loss window.
        if let Some(id) = Self::deletion_event_id(event_bytes) {
            let is_new = self
                .wal_ids
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(id);
            if is_new {
                let snapshot = {
                    let mut wal = self.wal.lock().unwrap_or_else(|p| p.into_inner());
                    wal.extend_from_slice(&(event_bytes.len() as u32).to_le_bytes());
                    wal.extend_from_slice(event_bytes);
                    let mut entries = self.wal_entries.lock().unwrap_or_else(|p| p.into_inner());
                    *entries += 1;
                    if *entries > MAX_WAL_ENTRIES {
                        Self::compact_wal(&mut wal, &mut entries, &self.wal_ids);
                    }
                    wal.clone()
                };
                self.blob_store.put(TOMBSTONES_KEY, &snapshot).await?;
            }
        }

        if self.should_sync() {
            self.sync_to_blob_store().await?;
            self.mark_synced();
        }
        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        self.core.initialize().await?;
        self.hydrate_from_blob_store().await?;
        // Tombstone replay comes last: referenced events must be indexed
        // before deletions can resolve them to keys.
        self.load_tombstones().await
    }

    fn get_relays(&self, request: &Request<'_>) -> Option<Vec<String>> {
        self.core.get_relays(request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generated::nostr::fb;
    use flatbuffers::FlatBufferBuilder;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct MemBlobStore {
        data: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    }

    #[async_trait(?Send)]
    impl BlobStore for MemBlobStore {
        async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
            Ok(self.data.lock().unwrap().get(key).cloned())
        }

        async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
            self.data
                .lock()
                .unwrap()
                .insert(key.to_string(), bytes.to_vec());
            Ok(())
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

    fn hex_id(n: usize) -> String {
        format!("{:064x}", n)
    }

    fn query_kind<B: BlobStore>(storage: &PersistentNostrDbStorage<B>, kind: u16) -> Vec<Vec<u8>> {
        let mut filter = Filter::new();
        filter.kinds = Some(vec![kind]);
        futures::executor::block_on(storage.query(vec![filter])).unwrap()
    }

    #[tokio::test]
    async fn tombstone_wal_survives_restart() {
        let blob = MemBlobStore::default();
        let author = hex_id(99);
        let target_id = hex_id(1);
        let deletion_id = hex_id(2);
        let address = format!("31923:{}:meetup-1", author);

        let target =
            build_parsed_worker_message(&target_id, &author, 31923, 1000, &[&["d", "meetup-1"]]);
        let deletion =
            build_parsed_worker_message(&deletion_id, &author, 5, 2000, &[&["a", &address]]);

        // Session 1: persist target + deletion.
        let storage1 = PersistentNostrDbStorage::new(
            NostrDbStorage::new("wal-test".to_string(), 1024 * 1024, vec![], vec![]),
            blob.clone(),
        );
        storage1.initialize().await.unwrap();
        storage1.persist(&target).await.unwrap();
        storage1.persist(&deletion).await.unwrap();

        // Filtered immediately, and the WAL hit the blob store eagerly.
        assert!(query_kind(&storage1, 31923).is_empty());
        let wal_len = blob.data.lock().unwrap()["tombstones"].len();
        assert!(wal_len > 4 + deletion.len() - 1);

        // Force the shard snapshot (normally on the 30s sync tick), then
        // simulate a restart with a fresh storage over the same blob data.
        storage1.sync_to_blob_store().await.unwrap();
        let storage2 = PersistentNostrDbStorage::new(
            NostrDbStorage::new("wal-test".to_string(), 1024 * 1024, vec![], vec![]),
            blob.clone(),
        );
        storage2.initialize().await.unwrap();

        // The target was hydrated from shards (the deletion event proves the
        // snapshot had data), but tombstone replay keeps it filtered.
        assert_eq!(
            query_kind(&storage2, 5).len(),
            1,
            "shard snapshot should contain the kind 5"
        );
        assert!(
            query_kind(&storage2, 31923).is_empty(),
            "WAL replay must filter the target"
        );

        // The loaded WAL dedupes: re-persisting the same deletion must not grow it.
        storage2.persist(&deletion).await.unwrap();
        assert_eq!(blob.data.lock().unwrap()["tombstones"].len(), wal_len);

        // A new deletion appends to the loaded WAL (old entries retained).
        let deletion2 =
            build_parsed_worker_message(&hex_id(3), &author, 5, 3000, &[&["e", &target_id]]);
        storage2.persist(&deletion2).await.unwrap();
        assert!(blob.data.lock().unwrap()["tombstones"].len() > wal_len);
    }

    #[tokio::test]
    async fn missing_tombstone_blob_is_fine() {
        let blob = MemBlobStore::default();
        let storage = PersistentNostrDbStorage::new(
            NostrDbStorage::new("wal-empty".to_string(), 1024 * 1024, vec![], vec![]),
            blob,
        );
        storage.initialize().await.unwrap();
        assert!(storage.load_tombstones().await.is_ok());
    }

    #[tokio::test]
    async fn ephemeral_events_are_available_only_for_the_current_session() {
        let blob = MemBlobStore::default();
        let event = build_parsed_worker_message(&hex_id(10), &hex_id(11), 20001, 1000, &[]);
        let storage1 = PersistentNostrDbStorage::new(
            NostrDbStorage::new("ephemeral-test".to_string(), 1024 * 1024, vec![], vec![]),
            blob.clone(),
        );
        storage1.initialize().await.unwrap();
        storage1.persist(&event).await.unwrap();

        assert_eq!(query_kind(&storage1, 20001).len(), 1);
        storage1.sync_to_blob_store().await.unwrap();
        assert!(!blob.data.lock().unwrap().contains_key("shard:ephemeral"));

        let storage2 = PersistentNostrDbStorage::new(
            NostrDbStorage::new("ephemeral-test".to_string(), 1024 * 1024, vec![], vec![]),
            blob,
        );
        storage2.initialize().await.unwrap();
        assert!(query_kind(&storage2, 20001).is_empty());
    }

    #[tokio::test]
    async fn legacy_kind10002_blob_is_folded_into_replaceable_shard() {
        let blob = MemBlobStore::default();
        let event = build_parsed_worker_message(&hex_id(20), &hex_id(21), 10002, 1000, &[]);

        let legacy_source =
            NostrDbStorage::new("legacy-source".to_string(), 1024 * 1024, vec![], vec![]);
        legacy_source.initialize().await.unwrap();
        legacy_source.persist(&event).await.unwrap();
        let legacy_bytes = legacy_source
            .sharded_storage()
            .save_all_shards()
            .remove(&ShardId::Replaceable)
            .expect("kind 10002 should route to replaceable storage");
        blob.data
            .lock()
            .unwrap()
            .insert("shard:kind10002".to_string(), legacy_bytes);

        let storage = PersistentNostrDbStorage::new(
            NostrDbStorage::new("legacy-target".to_string(), 1024 * 1024, vec![], vec![]),
            blob.clone(),
        );
        storage.initialize().await.unwrap();
        assert_eq!(query_kind(&storage, 10002).len(), 1);

        storage.sync_to_blob_store().await.unwrap();
        assert!(blob.data.lock().unwrap().contains_key("shard:replaceable"));
    }
}
