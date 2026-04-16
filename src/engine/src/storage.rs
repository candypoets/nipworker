use async_lock::Mutex;
use async_trait::async_trait;
use nipworker_core::traits::{Storage, StorageError};
use nipworker_core::types::nostr::Filter;
use serde_json::Value;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{
	IdbDatabase, IdbFactory, IdbRequest, IdbTransactionMode, IdbVersionChangeEvent,
};

const DB_NAME: &str = "nipworker-engine";
const DB_VERSION: u32 = 1;
const STORE_NAME: &str = "events";

fn indexed_db() -> Result<IdbFactory, StorageError> {
	let global = js_sys::global();
	let idb = js_sys::Reflect::get(&global, &"indexedDB".into())
		.map_err(|e| StorageError::Other(format!("indexed_db error: {:?}", e)))?;
	if idb.is_null() || idb.is_undefined() {
		return Err(StorageError::Other("IndexedDB not available".to_string()));
	}
	idb.dyn_into::<IdbFactory>()
		.map_err(|e| StorageError::Other(format!("Invalid IndexedDB factory: {:?}", e)))
}

async fn request_to_future(req: &IdbRequest) -> Result<JsValue, StorageError> {
	let req = req.clone();
	let promise = js_sys::Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
		let req_success = req.clone();
		let success = Closure::once_into_js(move || {
			let _ = resolve.call1(
				&JsValue::NULL,
				&req_success.result().unwrap_or(JsValue::NULL),
			);
		});
		let error = Closure::once_into_js(move || {
			let _ = reject.call0(&JsValue::NULL);
		});
		req.set_onsuccess(Some(success.as_ref().unchecked_ref()));
		req.set_onerror(Some(error.as_ref().unchecked_ref()));
	});
	JsFuture::from(promise)
		.await
		.map_err(|e| StorageError::Other(format!("IndexedDB request failed: {:?}", e)))
}

/// Simple in-memory storage for WASM engine.
/// In production this should be backed by IndexedDB.
#[derive(Debug)]
pub struct MemoryStorage {
	events: async_lock::RwLock<std::collections::HashMap<String, Vec<u8>>>,
}

impl MemoryStorage {
	pub fn new() -> Self {
		Self {
			events: async_lock::RwLock::new(std::collections::HashMap::new()),
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

/// Persistent IndexedDB-backed storage for WASM engine.
#[derive(Debug)]
pub struct IndexedDbStorage {
	db: Mutex<Option<IdbDatabase>>,
}

impl IndexedDbStorage {
	pub fn new() -> Self {
		Self { db: Mutex::new(None) }
	}

	async fn get_db(&self) -> Result<IdbDatabase, StorageError> {
		let mut guard = self.db.lock().await;
		if let Some(db) = guard.as_ref() {
			return Ok(db.clone());
		}
		let db = self.open_db().await?;
		*guard = Some(db.clone());
		Ok(db)
	}

	async fn open_db(&self) -> Result<IdbDatabase, StorageError> {
		let factory = indexed_db()?;
		let req = factory
			.open_with_u32(DB_NAME, DB_VERSION)
			.map_err(|e| StorageError::Other(format!("open error: {:?}", e)))?;

		let req_clone = req.clone();
		let upgrade = Closure::once_into_js(move |_event: IdbVersionChangeEvent| {
			if let Ok(res) = req_clone.result() {
				if let Ok(db) = res.dyn_into::<IdbDatabase>() {
					if !db.object_store_names().contains(STORE_NAME as &str) {
						let _ = db.create_object_store(STORE_NAME);
					}
				}
			}
		});
		req.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));

		let result = request_to_future(&req).await?;
		result
			.dyn_into::<IdbDatabase>()
			.map_err(|e| StorageError::Other(format!("Invalid database: {:?}", e)))
	}
}

#[async_trait(?Send)]
impl Storage for IndexedDbStorage {
	async fn query(&self, _filters: Vec<Filter>) -> Result<Vec<Vec<u8>>, StorageError> {
		let db = self.get_db().await?;
		let tx = db
			.transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readonly)
			.map_err(|e| StorageError::Other(format!("transaction error: {:?}", e)))?;
		let store = tx
			.object_store(STORE_NAME)
			.map_err(|e| StorageError::Other(format!("object_store error: {:?}", e)))?;

		let request = store
			.get_all()
			.map_err(|e| StorageError::Other(format!("get_all error: {:?}", e)))?;

		let result = request_to_future(&request).await?;
		let array = js_sys::Array::from(&result);
		let mut events = Vec::new();
		for i in 0..array.length() {
			let val = array.get(i);
			if let Ok(arr) = val.clone().dyn_into::<js_sys::Uint8Array>() {
				events.push(arr.to_vec());
			} else if let Ok(buf) = val.dyn_into::<js_sys::ArrayBuffer>() {
				let arr = js_sys::Uint8Array::new(&buf);
				events.push(arr.to_vec());
			}
		}
		Ok(events)
	}

	async fn persist(&self, event_bytes: &[u8]) -> Result<(), StorageError> {
		let id = extract_event_id(event_bytes)?;
		let db = self.get_db().await?;
		let tx = db
			.transaction_with_str_and_mode(STORE_NAME, IdbTransactionMode::Readwrite)
			.map_err(|e| StorageError::Other(format!("transaction error: {:?}", e)))?;
		let store = tx
			.object_store(STORE_NAME)
			.map_err(|e| StorageError::Other(format!("object_store error: {:?}", e)))?;

		let key = JsValue::from_str(&id);
		let value = js_sys::Uint8Array::from(event_bytes);
		let request = store
			.put_with_key(&value, &key)
			.map_err(|e| StorageError::Other(format!("put error: {:?}", e)))?;

		request_to_future(&request).await?;
		Ok(())
	}

	async fn initialize(&self) -> Result<(), StorageError> {
		let _ = self.get_db().await?;
		Ok(())
	}
}

fn extract_event_id(event_bytes: &[u8]) -> Result<String, StorageError> {
	let json: Value = serde_json::from_slice(event_bytes)
		.map_err(|e| StorageError::Other(format!("JSON parse error: {}", e)))?;
	json.get("id")
		.and_then(|v| v.as_str())
		.map(|s| s.to_string())
		.ok_or_else(|| StorageError::Other("Event missing id field".to_string()))
}
