//! IndexedDB blob storage for persisted ring-buffer shards.
//!
//! Architecture:
//! - One database per logical DB name: "{logical_name}-ringbuffer"
//! - One object store: "blobs"
//! - Keys are shared shard keys like "shard:default" and values are raw bytes

use crate::idb_utils::{get_idb_factory, idb_open_request_promise, idb_request_promise};
use js_sys::Uint8Array;
use nipworker_core::storage::BlobStore;
use nipworker_core::traits::StorageError;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbTransactionMode, IdbVersionChangeEvent};

const BLOBS_STORE: &str = "blobs";

pub struct IndexedDbBlobStore {
    logical_name: String,
}

impl IndexedDbBlobStore {
    pub fn new(logical_name: String) -> Self {
        Self { logical_name }
    }

    fn db_name(&self) -> String {
        format!("{}-ringbuffer", self.logical_name)
    }

    async fn open_db(&self) -> Result<IdbDatabase, StorageError> {
        let idb_factory =
            get_idb_factory().map_err(|_| StorageError::Other("IndexedDB not available".into()))?;

        let open_request = idb_factory
            .open_with_u32(&self.db_name(), 2)
            .map_err(|_| StorageError::Other("Failed to open IndexedDB database".into()))?;

        let upgrade = Closure::once(move |event: IdbVersionChangeEvent| {
            let target = event.target().unwrap();
            let request: web_sys::IdbOpenDbRequest = target.dyn_into().unwrap();
            let result = request.result().unwrap();
            let db: IdbDatabase = result.dyn_into().unwrap();

            if !db.object_store_names().contains(BLOBS_STORE) {
                db.create_object_store(BLOBS_STORE)
                    .expect("Failed to create IndexedDB blob store");
            }
        });

        open_request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        upgrade.forget();

        let db_js = JsFuture::from(idb_open_request_promise(&open_request))
            .await
            .map_err(|e| {
                StorageError::Other(format!("Failed to open IndexedDB database: {:?}", e))
            })?;

        db_js
            .dyn_into::<IdbDatabase>()
            .map_err(|_| StorageError::Other("Failed to cast IndexedDB database".into()))
    }

    async fn get_legacy_blob(
        &self,
        db: &IdbDatabase,
        key: &str,
    ) -> Result<Option<Vec<u8>>, StorageError> {
        if !db.object_store_names().contains(key) {
            return Ok(None);
        }

        let tx = db
            .transaction_with_str_and_mode(key, IdbTransactionMode::Readonly)
            .map_err(|_| {
                StorageError::Other("Failed to create legacy IndexedDB read transaction".into())
            })?;
        let store = tx.object_store(key).map_err(|_| {
            StorageError::Other("Failed to get legacy IndexedDB shard store".into())
        })?;
        let request = store
            .get(&JsValue::from_str("buffer"))
            .map_err(|_| StorageError::Other("Failed to get legacy IndexedDB shard blob".into()))?;

        let value = JsFuture::from(idb_request_promise(&request))
            .await
            .map_err(|e| StorageError::Other(format!("Legacy IndexedDB get failed: {:?}", e)))?;

        Self::js_value_to_bytes(value)
    }

    fn js_value_to_bytes(value: JsValue) -> Result<Option<Vec<u8>>, StorageError> {
        if value.is_undefined() || value.is_null() {
            return Ok(None);
        }

        if let Ok(arr) = value.clone().dyn_into::<Uint8Array>() {
            return Ok(Some(arr.to_vec()));
        }

        if let Ok(buf) = value.dyn_into::<js_sys::ArrayBuffer>() {
            return Ok(Some(Uint8Array::new(&buf).to_vec()));
        }

        Ok(None)
    }
}

#[async_trait::async_trait(?Send)]
impl BlobStore for IndexedDbBlobStore {
    async fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let db = self.open_db().await?;
        let tx = db
            .transaction_with_str_and_mode(BLOBS_STORE, IdbTransactionMode::Readonly)
            .map_err(|_| {
                StorageError::Other("Failed to create IndexedDB read transaction".into())
            })?;
        let store = tx
            .object_store(BLOBS_STORE)
            .map_err(|_| StorageError::Other("Failed to get IndexedDB blob store".into()))?;
        let request = store
            .get(&JsValue::from_str(key))
            .map_err(|_| StorageError::Other("Failed to get IndexedDB blob".into()))?;

        let value = JsFuture::from(idb_request_promise(&request))
            .await
            .map_err(|e| StorageError::Other(format!("IndexedDB get failed: {:?}", e)))?;

        if let Some(bytes) = Self::js_value_to_bytes(value)? {
            return Ok(Some(bytes));
        }

        self.get_legacy_blob(&db, key).await
    }

    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        let db = self.open_db().await?;
        let tx = db
            .transaction_with_str_and_mode(BLOBS_STORE, IdbTransactionMode::Readwrite)
            .map_err(|_| {
                StorageError::Other("Failed to create IndexedDB write transaction".into())
            })?;
        let store = tx
            .object_store(BLOBS_STORE)
            .map_err(|_| StorageError::Other("Failed to get IndexedDB blob store".into()))?;

        let js_array = Uint8Array::from(bytes);
        let request = store
            .put_with_key(&js_array, &JsValue::from_str(key))
            .map_err(|_| StorageError::Other("Failed to put IndexedDB blob".into()))?;
        JsFuture::from(idb_request_promise(&request))
            .await
            .map_err(|e| StorageError::Other(format!("IndexedDB put failed: {:?}", e)))?;
        Ok(())
    }
}
