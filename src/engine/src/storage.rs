use async_trait::async_trait;
use nipworker_core::storage::db::sharded_storage::ShardId;
use nipworker_core::storage::NostrDbStorage as CoreNostrDbStorage;
use nipworker_core::traits::{Storage, StorageError};
use nipworker_core::types::nostr::Filter;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::ring_buffer_persist::IndexedDbRingBufferPersistence;

const DEFAULT_RELAYS: &[&str] = &["wss://relay.snort.social", "wss://relay.damus.io", "wss://relay.primal.net"];
const INDEXER_RELAYS: &[&str] = &["wss://user.kindpag.es", "wss://relay.nos.social", "wss://purplepag.es", "wss://relay.nostr.band"];

// 30 seconds in milliseconds
const SYNC_INTERVAL_MS: i64 = 30_000;

/// WASM-aware NostrDbStorage that adds IndexedDB ring-buffer persistence.
/// 
/// Architecture:
/// - Core `NostrDbStorage` provides fast in-memory queries via NostrDB indexes
/// - IndexedDB is used only for ring buffer persistence (hydration on init, periodic save)
/// - 30-second sync timer: only saves to IndexedDB if 30s has elapsed since last sync
pub struct NostrDbStorage {
    core: Rc<CoreNostrDbStorage>,
    last_sync_time: Cell<i64>,
}

impl NostrDbStorage {
    pub fn new(max_buffer_size: usize) -> Self {
        let default_relays: Vec<String> = DEFAULT_RELAYS.iter().map(|s| s.to_string()).collect();
        let indexer_relays: Vec<String> = INDEXER_RELAYS.iter().map(|s| s.to_string()).collect();
        
        let core = Rc::new(CoreNostrDbStorage::new(
            "nipworker".to_string(),
            max_buffer_size,
            default_relays,
            indexer_relays,
        ));
        
        Self {
            core,
            last_sync_time: Cell::new(0),
        }
    }

    /// Get the current timestamp in milliseconds
    fn now_ms() -> i64 {
        let now = js_sys::Date::new_0();
        now.get_time() as i64
    }

    /// Check if we should sync (30 seconds elapsed)
    fn should_sync(&self) -> bool {
        let now = Self::now_ms();
        let last = self.last_sync_time.get();
        now - last > SYNC_INTERVAL_MS
    }

    /// Update last sync time to now
    fn mark_synced(&self) {
        self.last_sync_time.set(Self::now_ms());
    }

    /// Load ring buffers from IndexedDB into NostrDB
    async fn hydrate_from_indexeddb(&self) -> Result<(), StorageError> {
        // Get the sharded storage reference directly from core
        let sharded = self.core.sharded_storage();

        let shard_ids: Vec<ShardId> = IndexedDbRingBufferPersistence::get_shard_ids(sharded);
        let db_name: &str = sharded.db_name();

        web_sys::console::log_1(&JsValue::from_str("[NostrDbStorage] Hydrating from IndexedDB..."));

        let load_result: Result<HashMap<ShardId, Vec<u8>>, _> = IndexedDbRingBufferPersistence::load_all(db_name, &shard_ids).await;
        match load_result {
            Ok(shard_bytes) => {
                let shard_bytes: HashMap<ShardId, Vec<u8>> = shard_bytes;
                let count: usize = shard_bytes.len();
                if count > 0 {
                    web_sys::console::log_1(&JsValue::from_str(&format!(
                        "[NostrDbStorage] Loaded {} shards from IndexedDB",
                        count
                    )));
                    
                    // Load into ring buffers
                    if let Err(e) = sharded.load_all_shards(&shard_bytes) {
                        web_sys::console::warn_1(&JsValue::from_str(&format!(
                            "[NostrDbStorage] Failed to load shards: {}",
                            e
                        )));
                    } else {
                        web_sys::console::log_1(&JsValue::from_str(
                            "[NostrDbStorage] Shards loaded into memory"
                        ));
                    }
                } else {
                    web_sys::console::log_1(&JsValue::from_str(
                        "[NostrDbStorage] No persisted data found in IndexedDB"
                    ));
                }
            }
            Err(e) => {
                web_sys::console::warn_1(&JsValue::from_str(&format!(
                    "[NostrDbStorage] Failed to load from IndexedDB: {}",
                    e
                )));
            }
        }

        Ok(())
    }

    /// Save ring buffers to IndexedDB if 30s has elapsed
    async fn maybe_sync_to_indexeddb(&self) -> Result<(), StorageError> {
        // Check if we should sync
        if !self.should_sync() {
            return Ok(());
        }

        // Get the sharded storage reference
        let sharded = self.core.sharded_storage();

        let shard_ids: Vec<ShardId> = IndexedDbRingBufferPersistence::get_shard_ids(sharded);
        let db_name: &str = sharded.db_name();

        // Save all shards
        let shard_bytes: HashMap<ShardId, Vec<u8>> = sharded.save_all_shards();

        let count: usize = shard_bytes.len();
        if count > 0 {
            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[NostrDbStorage] Syncing {} shards to IndexedDB...",
                count
            )));

            match IndexedDbRingBufferPersistence::save_all(db_name, &shard_ids, &shard_bytes).await {
                Ok(()) => {
                    web_sys::console::log_1(&JsValue::from_str(
                        "[NostrDbStorage] Sync to IndexedDB complete"
                    ));
                    self.mark_synced();
                }
                Err(e) => {
                    web_sys::console::warn_1(&JsValue::from_str(&format!(
                        "[NostrDbStorage] Failed to sync to IndexedDB: {}",
                        e
                    )));
                }
            }
        }

        Ok(())
    }

    /// Spawn background sync task
    fn spawn_background_sync(&self) {
        let core_clone: Rc<CoreNostrDbStorage> = Rc::clone(&self.core);
        let last_sync_ref: Cell<i64> = self.last_sync_time.clone();
        
        spawn_local(async move {
            // Check if enough time has passed
            let now: i64 = Self::now_ms();
            let last: i64 = last_sync_ref.get();
            if now - last <= SYNC_INTERVAL_MS {
                return;
            }

            // Get sharded storage
            let sharded = core_clone.sharded_storage();

            let shard_ids: Vec<ShardId> = IndexedDbRingBufferPersistence::get_shard_ids(sharded);
            let db_name: &str = sharded.db_name();
            let shard_bytes: HashMap<ShardId, Vec<u8>> = sharded.save_all_shards();

            let count: usize = shard_bytes.len();
            if count == 0 {
                return;
            }

            web_sys::console::log_1(&JsValue::from_str(&format!(
                "[NostrDbStorage] Background sync of {} shards...",
                count
            )));
            
            let save_result: Result<(), _> = IndexedDbRingBufferPersistence::save_all(db_name, &shard_ids, &shard_bytes).await;
            if save_result.is_ok() {
                last_sync_ref.set(now);
                web_sys::console::log_1(&JsValue::from_str(
                    "[NostrDbStorage] Background sync complete"
                ));
            }
        });
    }
}

#[async_trait(?Send)]
impl Storage for NostrDbStorage {
    async fn query(&self, filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
        // Fast in-memory query via NostrDB indexes
        self.core.query(filters).await
    }

    async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
        // Persist to in-memory NostrDB
        self.core.persist(event_bytes).await?;

        // Check if we should sync to IndexedDB (30s timer)
        // Spawn fire-and-forget background task
        if self.should_sync() {
            self.spawn_background_sync();
        }

        Ok(())
    }

    async fn initialize(&self) -> Result<(), StorageError> {
        // Initialize core NostrDB
        self.core.initialize().await?;

        // Hydrate from IndexedDB if data exists
        self.hydrate_from_indexeddb().await?;

        Ok(())
    }
}
