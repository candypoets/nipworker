use crate::{
    db::types::{DatabaseConfig, DatabaseError, EventStorage},
    generated::nostr::fb,
    parsed_event::ParsedEvent,
};
use std::{
    cell::RefCell,
    sync::{Arc, RwLock},
};
use tracing::{debug, info};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbTransactionMode};

/// Simple ring buffer storage implementation using IndexedDB
#[derive(Debug, Clone)]
pub struct RingBufferStorage {
    db_name: String,
    buffer_key: String,
    max_buffer_size: usize,
    pending_events_counter: RefCell<u32>, // used to save the buffer every 50 events
    config: DatabaseConfig,
    /// The actual ring buffer - just raw bytes
    buffer: Arc<RwLock<Vec<u8>>>,
    /// Whether we've loaded from storage
    initialized: Arc<RwLock<bool>>,
}

impl RingBufferStorage {
    /// Create a new ring buffer storage instance
    pub fn new(
        db_name: String,
        buffer_key: String,
        max_buffer_size: usize,
        config: DatabaseConfig,
    ) -> Self {
        info!(
            "Creating ring buffer '{}' with max size {} bytes",
            buffer_key, max_buffer_size
        );
        Self {
            db_name,
            buffer_key,
            max_buffer_size,
            pending_events_counter: RefCell::new(0),
            config,
            buffer: Arc::new(RwLock::new(Vec::with_capacity(max_buffer_size))),
            initialized: Arc::new(RwLock::new(false)),
        }
    }

    /// Initialize by loading existing buffer from IndexedDB
    pub async fn initialize(&self) -> Result<(), DatabaseError> {
        let mut initialized = self
            .initialized
            .write()
            .map_err(|_| DatabaseError::LockError)?;

        if *initialized {
            return Ok(());
        }

        // Load existing buffer from IndexedDB
        let buffer_data = self.load_buffer_from_indexeddb().await?;

        if !buffer_data.is_empty() {
            let mut buffer = self.buffer.write().map_err(|_| DatabaseError::LockError)?;
            *buffer = buffer_data;

            info!(
                "Initialized ring buffer '{}' with {} bytes",
                self.buffer_key,
                buffer.len()
            );
        } else {
            info!("Initialized empty ring buffer '{}'", self.buffer_key);
        }

        *initialized = true;
        Ok(())
    }

    /// Add a new event to the ring buffer
    pub async fn add_event(&self, event_data: &[u8]) -> Result<(), DatabaseError> {
        info!(
            "Try Adding event to ring buffer '{}': {} bytes",
            self.buffer_key,
            event_data.len()
        );

        // Ensure initialized
        if !*self
            .initialized
            .read()
            .map_err(|_| DatabaseError::LockError)?
        {
            self.initialize().await?;
        }

        let event_size = event_data.len();
        let total_size = 4 + event_size; // 4 bytes for size prefix

        // Check if event is too large
        if total_size > self.max_buffer_size {
            return Err(DatabaseError::StorageError(format!(
                "Event too large ({} bytes) for buffer ({} bytes)",
                total_size, self.max_buffer_size
            )));
        }

        let mut buffer = self.buffer.write().map_err(|_| DatabaseError::LockError)?;

        // Remove old events from front until we have space
        while buffer.len() + total_size > self.max_buffer_size {
            if !self.remove_first_event(&mut buffer) {
                break; // No more events to remove
            }
        }

        // Add size prefix
        buffer.extend_from_slice(&(event_size as u32).to_le_bytes());
        // Add event data
        buffer.extend_from_slice(event_data);

        info!(
            "Added event to ring buffer '{}': size={}, total_buffer_size={}",
            self.buffer_key,
            event_size,
            buffer.len()
        );

        // Persist to IndexedDB
        drop(buffer);

        // Increment and check counter
        self.pending_events_counter.replace_with(|&mut old| old + 1);

        if *self.pending_events_counter.borrow() > 50 {
            self.persist_to_indexeddb().await?;
            self.pending_events_counter.replace(0);
        }

        Ok(())
    }

    /// Remove the first event from the buffer
    fn remove_first_event(&self, buffer: &mut Vec<u8>) -> bool {
        if buffer.len() < 4 {
            return false;
        }

        // Read the size of the first event
        let size = u32::from_le_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;

        let total_size = 4 + size;

        // Validate size
        if total_size > buffer.len() {
            // Corrupted buffer, clear it
            buffer.clear();
            return false;
        }

        // Remove the first event (size prefix + data)
        buffer.drain(0..total_size);

        debug!(
            "Removed first event from ring buffer '{}': freed {} bytes, remaining {} bytes",
            self.buffer_key,
            total_size,
            buffer.len()
        );

        true
    }

    /// Extract all events from the buffer
    fn extract_events_from_buffer(buffer: &[u8]) -> Vec<Vec<u8>> {
        let mut events = Vec::new();
        let mut offset = 0;

        while offset + 4 <= buffer.len() {
            // Read size prefix
            let size = u32::from_le_bytes([
                buffer[offset],
                buffer[offset + 1],
                buffer[offset + 2],
                buffer[offset + 3],
            ]) as usize;

            // Validate size
            if size == 0 || offset + 4 + size > buffer.len() {
                break;
            }

            // Extract event data (without size prefix)
            let event_start = offset + 4;
            let event_end = event_start + size;
            events.push(buffer[event_start..event_end].to_vec());

            offset = event_end;
        }

        events
    }

    /// Open or create the IndexedDB database using web-sys
    async fn open_db(&self) -> Result<IdbDatabase, DatabaseError> {
        // Get the global context (window or worker)
        let global = js_sys::global();

        // Get IndexedDB factory
        let idb_factory = if let Some(window) = web_sys::window() {
            window
                .indexed_db()
                .map_err(|_| DatabaseError::StorageError("IndexedDB not available".into()))?
                .ok_or_else(|| DatabaseError::StorageError("IndexedDB not supported".into()))?
        } else {
            // In worker context, use Reflect to get indexedDB
            use js_sys::Reflect;
            let idb_value =
                Reflect::get(&global, &JsValue::from_str("indexedDB")).map_err(|_| {
                    DatabaseError::StorageError("IndexedDB not available in worker".into())
                })?;
            idb_value
                .dyn_into::<web_sys::IdbFactory>()
                .map_err(|_| DatabaseError::StorageError("Failed to cast to IdbFactory".into()))?
        };

        let open_request = idb_factory
            .open_with_u32(&self.db_name, 1)
            .map_err(|_| DatabaseError::StorageError("Failed to open database".into()))?;

        // Set up upgrade handler
        let store_name = "ring_buffers".to_string();
        let onupgradeneeded = Closure::once(move |event: web_sys::IdbVersionChangeEvent| {
            let target = event.target().unwrap();
            let request: IdbOpenDbRequest = target.dyn_into().unwrap();
            let result = request.result().unwrap();
            let db: IdbDatabase = result.dyn_into().unwrap();

            // Create object store if it doesn't exist
            if !db.object_store_names().contains(&store_name) {
                db.create_object_store(&store_name)
                    .expect("Failed to create object store");
                // No indexes needed for our simple key-value use case
            }
        });

        open_request.set_onupgradeneeded(Some(onupgradeneeded.as_ref().unchecked_ref()));
        onupgradeneeded.forget();

        // Convert IdbOpenDbRequest to Promise for JsFuture
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let open_request_clone = open_request.clone();
            let resolve_clone = resolve.clone();
            let success = Closure::once(move || {
                let db = open_request_clone.result().unwrap();
                resolve_clone.call1(&JsValue::NULL, &db).unwrap();
            });
            let reject_clone = reject.clone();
            let error = Closure::once(move || {
                reject_clone
                    .call1(
                        &JsValue::NULL,
                        &JsValue::from_str("Failed to open database"),
                    )
                    .unwrap();
            });
            open_request.set_onsuccess(Some(success.as_ref().unchecked_ref()));
            open_request.set_onerror(Some(error.as_ref().unchecked_ref()));
            success.forget();
            error.forget();
        });

        // Convert to future and await
        let db = JsFuture::from(promise).await.map_err(|e| {
            DatabaseError::StorageError(format!("Failed to open database: {:?}", e))
        })?;

        db.dyn_into::<IdbDatabase>()
            .map_err(|_| DatabaseError::StorageError("Failed to cast to IdbDatabase".into()))
    }

    /// Persist current buffer to IndexedDB
    async fn persist_to_indexeddb(&self) -> Result<(), DatabaseError> {
        let buffer_data = self
            .buffer
            .read()
            .map_err(|_| DatabaseError::LockError)?
            .clone();

        if buffer_data.is_empty() {
            return Ok(());
        }

        let db = self.open_db().await?;

        // Create transaction
        let transaction = db
            .transaction_with_str_and_mode("ring_buffers", IdbTransactionMode::Readwrite)
            .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;

        let store = transaction
            .object_store("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;

        // Store the buffer with the key
        let js_array = js_sys::Uint8Array::from(&buffer_data[..]);

        store
            .put_with_key(&js_array, &JsValue::from_str(&self.buffer_key))
            .map_err(|_| DatabaseError::StorageError("Failed to put data".into()))?;

        Ok(())
    }

    /// Load buffer from IndexedDB
    async fn load_buffer_from_indexeddb(&self) -> Result<Vec<u8>, DatabaseError> {
        let db = self.open_db().await?;

        // Create transaction
        let transaction = db
            .transaction_with_str("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;

        let store = transaction
            .object_store("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;

        let get_request = store
            .get(&JsValue::from_str(&self.buffer_key))
            .map_err(|_| DatabaseError::StorageError("Failed to get data".into()))?;

        // Convert IdbRequest to Promise for JsFuture
        let promise = js_sys::Promise::new(&mut |resolve, reject| {
            let get_request_clone = get_request.clone();
            let success = Closure::once(move || {
                let result = get_request_clone.result().unwrap();
                resolve.call1(&JsValue::NULL, &result).unwrap();
            });
            let error = Closure::once(move || {
                reject
                    .call1(&JsValue::NULL, &JsValue::from_str("Failed to get data"))
                    .unwrap();
            });
            get_request.set_onsuccess(Some(success.as_ref().unchecked_ref()));
            get_request.set_onerror(Some(error.as_ref().unchecked_ref()));
            success.forget();
            error.forget();
        });

        let result = JsFuture::from(promise)
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Get operation failed: {:?}", e)))?;

        // Check if we got data
        if result.is_null() || result.is_undefined() {
            return Ok(Vec::new());
        }

        // Convert to Vec<u8>
        if let Ok(uint8_array) = result.dyn_into::<js_sys::Uint8Array>() {
            let mut buffer = vec![0u8; uint8_array.length() as usize];
            uint8_array.copy_to(&mut buffer);
            Ok(buffer)
        } else {
            Ok(Vec::new())
        }
    }
}

impl EventStorage for RingBufferStorage {
    async fn add_event_data(&self, event_data: &[u8]) -> Result<(), DatabaseError> {
        info!(
            "Adding event to ring buffer '{}': {} bytes",
            self.buffer_key,
            event_data.len()
        );
        self.add_event(event_data).await.map_err(|e| {
            tracing::error!(
                "Failed to add event to ring buffer '{}': {}",
                self.buffer_key,
                e
            );
            e
        })
    }

    async fn load_events(&self) -> Result<Vec<Vec<u8>>, DatabaseError> {
        // Ensure initialized
        if !*self
            .initialized
            .read()
            .map_err(|_| DatabaseError::LockError)?
        {
            self.initialize().await?;
        }

        // Extract all events from the buffer
        let buffer = self.buffer.read().map_err(|_| DatabaseError::LockError)?;
        let events = Self::extract_events_from_buffer(&buffer);

        info!(
            "Loaded {} events from ring buffer '{}' ({} bytes total)",
            events.len(),
            self.buffer_key,
            buffer.len()
        );

        Ok(events)
    }

    async fn clear_storage(&self) -> Result<(), DatabaseError> {
        // Clear in-memory buffer
        {
            let mut buffer = self.buffer.write().map_err(|_| DatabaseError::LockError)?;
            buffer.clear();
        }

        // Clear from IndexedDB
        let db = self.open_db().await?;

        let transaction = db
            .transaction_with_str_and_mode("ring_buffers", IdbTransactionMode::Readwrite)
            .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;

        let store = transaction
            .object_store("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;

        store
            .delete(&JsValue::from_str(&self.buffer_key))
            .map_err(|_| DatabaseError::StorageError("Failed to delete data".into()))?;

        info!("Cleared ring buffer '{}'", self.buffer_key);
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Default factory
impl Default for RingBufferStorage {
    fn default() -> Self {
        Self::new(
            "nostr-ring-buffer-db".to_string(),
            "default".to_string(),
            10_000_000, // 10MB default
            DatabaseConfig::default(),
        )
    }
}
