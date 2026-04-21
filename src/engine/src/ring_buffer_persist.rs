//! IndexedDB persistence for ShardedRingBufferStorage
//! 
//! Architecture:
//! - One database per logical DB name: "{logical_name}-ringbuffer"
//! - One object store per shard: "shard:default", "shard:kind0", etc.
//! - Single key "buffer" holds the raw bytes

use crate::idb_utils::{get_idb_factory, idb_open_request_promise, idb_request_promise};
use js_sys::Uint8Array;
use nipworker_core::storage::db::sharded_storage::{ShardId, ShardedRingBufferStorage};
use nipworker_core::storage::db::types::DatabaseError;
use std::collections::HashMap;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    IdbDatabase, IdbTransactionMode, IdbVersionChangeEvent,
};

pub struct IndexedDbRingBufferPersistence;

impl IndexedDbRingBufferPersistence {
    /// Get the database name for a logical database
    fn db_name(logical_name: &str) -> String {
        format!("{}-ringbuffer", logical_name)
    }

    /// Get the object store name for a shard
    fn store_name(shard_id: ShardId) -> String {
        format!("shard:{:?}", shard_id).to_lowercase()
    }

    /// Open the IndexedDB database
    async fn open_db(logical_name: &str, shards: &[ShardId]) -> Result<IdbDatabase, DatabaseError> {
        let idb_factory = get_idb_factory()
            .map_err(|_| DatabaseError::StorageError("IndexedDB not available".into()))?;

        let db_name = Self::db_name(logical_name);
        let open_request = idb_factory
            .open_with_u32(&db_name, 1)
            .map_err(|_| DatabaseError::StorageError("Failed to open database".into()))?;

        // Set up upgrade handler
        let shards_for_upgrade: Vec<ShardId> = shards.to_vec();
        let upgrade = Closure::once(move |event: IdbVersionChangeEvent| {
            let target = event.target().unwrap();
            let request: web_sys::IdbOpenDbRequest = target.dyn_into().unwrap();
            let result = request.result().unwrap();
            let db: IdbDatabase = result.dyn_into().unwrap();

            for shard in &shards_for_upgrade {
                let store_name = Self::store_name(*shard);
                if !db.object_store_names().contains(&store_name) {
                    db.create_object_store(&store_name)
                        .expect("Failed to create object store");
                }
            }
        });

        open_request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        upgrade.forget();

        // Await open
        let db_js = JsFuture::from(idb_open_request_promise(&open_request))
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Failed to open database: {:?}", e)))?;

        db_js.dyn_into::<IdbDatabase>()
            .map_err(|_| DatabaseError::StorageError("Failed to cast to IdbDatabase".into()))
    }

    /// Load all shards from IndexedDB
    pub async fn load_all(
        logical_name: &str,
        shards: &[ShardId],
    ) -> Result<HashMap<ShardId, Vec<u8>>, DatabaseError> {
        let db = Self::open_db(logical_name, shards).await?;
        let mut result = HashMap::new();

        for shard in shards {
            let store_name = Self::store_name(*shard);
            
            if !db.object_store_names().contains(&store_name) {
                continue;
            }

            let tx = db
                .transaction_with_str_and_mode(&store_name, IdbTransactionMode::Readonly)
                .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;
            let store = tx
                .object_store(&store_name)
                .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;
            let request = store
                .get(&JsValue::from_str("buffer"))
                .map_err(|_| DatabaseError::StorageError("Failed to get data".into()))?;

            let value = JsFuture::from(idb_request_promise(&request))
                .await
                .map_err(|_| DatabaseError::StorageError("Request failed".into()))?;

            if let Ok(arr) = value.clone().dyn_into::<Uint8Array>() {
                let bytes = arr.to_vec();
                if !bytes.is_empty() {
                    result.insert(*shard, bytes);
                }
            } else if let Ok(buf) = value.dyn_into::<js_sys::ArrayBuffer>() {
                let arr = Uint8Array::new(&buf);
                let bytes = arr.to_vec();
                if !bytes.is_empty() {
                    result.insert(*shard, bytes);
                }
            }
        }

        Ok(result)
    }

    /// Save all shards to IndexedDB
    pub async fn save_all(
        logical_name: &str,
        shards: &[ShardId],
        data: &HashMap<ShardId, Vec<u8>>,
    ) -> Result<(), DatabaseError> {
        let db = Self::open_db(logical_name, shards).await?;

        for (shard, bytes) in data {
            let store_name = Self::store_name(*shard);
            
            if !db.object_store_names().contains(&store_name) {
                continue;
            }

            let tx = db
                .transaction_with_str_and_mode(&store_name, IdbTransactionMode::Readwrite)
                .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;
            let store = tx
                .object_store(&store_name)
                .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;

            let js_array = Uint8Array::from(bytes.as_slice());
            store
                .put_with_key(&js_array, &JsValue::from_str("buffer"))
                .map_err(|_| DatabaseError::StorageError("Failed to put data".into()))?;
        }

        Ok(())
    }

    /// Get all shard IDs
    pub fn get_shard_ids(_storage: &ShardedRingBufferStorage) -> Vec<ShardId> {
        vec![
            ShardId::Default,
            ShardId::Kind0,
            ShardId::Kind4,
            ShardId::Kind7375,
            ShardId::Kind10002,
        ]
    }
}
