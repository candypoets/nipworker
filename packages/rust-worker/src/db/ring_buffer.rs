use crate::{
    db::types::{DatabaseConfig, DatabaseError, EventStorage},
    utils::js_interop::{
        get_idb_factory, idb_open_request_promise, idb_request_promise, uint8array_from_slice,
    },
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tracing::{debug, info};
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbTransactionMode};

const PERSIST_EVERY_N_EVENTS: u32 = 25; // set to 15 if you want more frequent flushes

/// Simple ring buffer storage implementation using IndexedDB
#[derive(Debug, Clone)]
pub struct RingBufferStorage {
    db_name: String,
    buffer_key: String,
    max_buffer_size: usize,
    // count of pending events before we flush to IndexedDB
    pending_events_counter: Cell<u32>,
    config: DatabaseConfig,
    /// The actual ring buffer - just raw bytes
    buffer: Rc<RefCell<Vec<u8>>>,
    /// Whether we've loaded from storage
    initialized: Cell<bool>,
    /// Total number of bytes logically removed from the front of the buffer over time.
    /// This lets us expose stable "global" offsets for events and detect outdated offsets.
    head_offset: Cell<u64>,
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
            pending_events_counter: Cell::new(0),
            config,
            buffer: Rc::new(RefCell::new(Vec::with_capacity(max_buffer_size))),
            initialized: Cell::new(false),
            head_offset: Cell::new(0),
        }
    }

    /// Initialize by loading existing buffer from IndexedDB
    pub async fn initialize(&self) -> Result<(), DatabaseError> {
        if self.initialized.get() {
            return Ok(());
        }

        // Load existing buffer from IndexedDB
        let buffer_data = self.load_buffer_from_indexeddb().await?;

        if !buffer_data.is_empty() {
            let mut buf = self.buffer.borrow_mut();
            *buf = buffer_data;

            info!(
                "Initialized ring buffer '{}' with {} bytes",
                self.buffer_key,
                buf.len()
            );
        } else {
            info!("Initialized empty ring buffer '{}'", self.buffer_key);
        }

        self.initialized.set(true);
        Ok(())
    }

    /// Add a new event to the ring buffer
    pub async fn add_event(&self, event_data: &[u8]) -> Result<u64, DatabaseError> {
        let event_size = event_data.len();
        let total_size = 4 + event_size; // 4 bytes for size prefix

        // Check if event is too large
        if total_size > self.max_buffer_size {
            return Err(DatabaseError::StorageError(format!(
                "Event too large ({} bytes) for buffer ({} bytes)",
                total_size, self.max_buffer_size
            )));
        }

        let mut buffer = self.buffer.borrow_mut();

        // Evict in one pass to make space (avoids repeated memmove on Vec::drain)
        self.evict_to_fit(&mut buffer, total_size);

        // Compute the global offset for the new event (before we append).
        let new_event_offset = self.head_offset.get() + buffer.len() as u64;

        // Add size prefix
        buffer.extend_from_slice(&(event_size as u32).to_le_bytes());
        // Add event data
        buffer.extend_from_slice(event_data);

        drop(buffer); // release borrow before await

        // Increment and check counter
        let next_count = self.pending_events_counter.get() + 1;
        self.pending_events_counter.set(next_count);

        if next_count >= PERSIST_EVERY_N_EVENTS {
            self.persist_to_indexeddb().await?;
            self.pending_events_counter.set(0);
        }

        Ok(new_event_offset)
    }

    /// Get event bytes at a given global offset (synchronous).
    /// Returns None if the offset is evicted, misaligned, or incomplete.
    pub fn get_event_at_offset(&self, offset: u64) -> Result<Option<Vec<u8>>, DatabaseError> {
        if !self.initialized.get() {
            return Err(DatabaseError::NotInitialized);
        }

        let head = self.head_offset.get();

        // If the requested offset is before the current head, it's been evicted
        if offset < head {
            return Ok(None);
        }

        let rel = (offset - head) as usize;
        let buffer = self.buffer.borrow();

        // Not available (beyond the tail or not enough bytes for size prefix)
        if rel + 4 > buffer.len() {
            return Ok(None);
        }

        // Read size prefix
        let size = u32::from_le_bytes([
            buffer[rel],
            buffer[rel + 1],
            buffer[rel + 2],
            buffer[rel + 3],
        ]) as usize;

        if size == 0 {
            return Ok(None);
        }

        let data_start = rel + 4;
        let data_end = data_start + size;

        // Validate bounds
        if data_end > buffer.len() {
            return Ok(None);
        }

        Ok(Some(buffer[data_start..data_end].to_vec()))
    }

    /// Remove the first event from the buffer (single event eviction)
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

        // Advance the global head offset by the number of bytes removed
        self.head_offset
            .set(self.head_offset.get() + total_size as u64);

        debug!(
            "Removed first event from ring buffer '{}': freed {} bytes, remaining {} bytes",
            self.buffer_key,
            total_size,
            buffer.len()
        );

        true
    }

    /// Evict enough events (in one drain) to make room for `needed` bytes.
    fn evict_to_fit(&self, buffer: &mut Vec<u8>, needed: usize) {
        let cur_len = buffer.len();
        if cur_len + needed <= self.max_buffer_size {
            return;
        }

        let mut must_free = cur_len + needed - self.max_buffer_size;

        // Scan events from the front, summing complete events until we free enough.
        let mut p = 0usize;
        while must_free > 0 && p + 4 <= cur_len {
            let size = u32::from_le_bytes([buffer[p], buffer[p + 1], buffer[p + 2], buffer[p + 3]])
                as usize;
            let total = 4 + size;

            if total > cur_len - p {
                // Corrupted/incomplete; clear everything for safety
                p = cur_len;
                break;
            }

            p += total;
            if must_free >= total {
                must_free -= total;
            } else {
                must_free = 0;
            }
        }

        if p > 0 {
            buffer.drain(0..p);

            // bump head_offset once
            self.head_offset.set(self.head_offset.get() + p as u64);

            debug!(
                "Evicted {} bytes to fit {}; remaining buffer={}",
                p,
                needed,
                buffer.len()
            );
        }
    }

    /// Extract all event OFFSETS (global offsets) from the buffer
    fn extract_events_from_buffer(&self) -> Vec<u64> {
        let buffer = self.buffer.borrow();

        let head = self.head_offset.get();
        let mut offsets = Vec::new();
        let mut p = 0usize;

        info!(
            "Buffer '{}' length: {} bytes",
            self.buffer_key,
            buffer.len()
        );
        while p + 4 <= buffer.len() {
            // Read size prefix
            let size = u32::from_le_bytes([buffer[p], buffer[p + 1], buffer[p + 2], buffer[p + 3]])
                as usize;

            // Validate size
            if size == 0 || p + 4 + size > buffer.len() {
                break;
            }

            // Global offset = head + local position
            offsets.push(head + p as u64);

            // Advance to next event
            p += 4 + size;
        }

        offsets
    }

    /// Open or create the IndexedDB database using web-sys
    async fn open_db(&self) -> Result<IdbDatabase, DatabaseError> {
        let idb_factory = get_idb_factory()
            .map_err(|_| DatabaseError::StorageError("IndexedDB not available".into()))?;

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

            if !db.object_store_names().contains(&store_name) {
                db.create_object_store(&store_name)
                    .expect("Failed to create object store");
            }
        });

        open_request.set_onupgradeneeded(Some(onupgradeneeded.as_ref().unchecked_ref()));
        onupgradeneeded.forget();

        // Await open
        let db_js = JsFuture::from(idb_open_request_promise(&open_request))
            .await
            .map_err(|e| {
                DatabaseError::StorageError(format!("Failed to open database: {:?}", e))
            })?;

        db_js
            .dyn_into::<IdbDatabase>()
            .map_err(|_| DatabaseError::StorageError("Failed to cast to IdbDatabase".into()))
    }

    /// Persist current buffer to IndexedDB
    async fn persist_to_indexeddb(&self) -> Result<(), DatabaseError> {
        let buffer_data = self.buffer.borrow().clone();

        if buffer_data.is_empty() {
            return Ok(());
        }

        let db = self.open_db().await?;

        let transaction = db
            .transaction_with_str_and_mode("ring_buffers", IdbTransactionMode::Readwrite)
            .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;

        let store = transaction
            .object_store("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;

        let js_array = uint8array_from_slice(&buffer_data);

        store
            .put_with_key(&js_array, &JsValue::from_str(&self.buffer_key))
            .map_err(|_| DatabaseError::StorageError("Failed to put data".into()))?;

        Ok(())
    }

    /// Load buffer from IndexedDB
    async fn load_buffer_from_indexeddb(&self) -> Result<Vec<u8>, DatabaseError> {
        let db = self.open_db().await?;

        let transaction = db
            .transaction_with_str("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to create transaction".into()))?;

        let store = transaction
            .object_store("ring_buffers")
            .map_err(|_| DatabaseError::StorageError("Failed to get object store".into()))?;

        let get_request = store
            .get(&JsValue::from_str(&self.buffer_key))
            .map_err(|_| DatabaseError::StorageError("Failed to get data".into()))?;

        let result = JsFuture::from(idb_request_promise(&get_request))
            .await
            .map_err(|e| DatabaseError::StorageError(format!("Get operation failed: {:?}", e)))?;

        if result.is_null() || result.is_undefined() {
            return Ok(Vec::new());
        }

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
    async fn initialize_storage(&self) -> Result<(), DatabaseError> {
        self.initialize().await
    }

    async fn add_event_data(&self, event_data: &[u8]) -> Result<u64, DatabaseError> {
        self.add_event(event_data).await.map_err(|e| {
            tracing::error!(
                "Failed to add event to ring buffer '{}': {}",
                self.buffer_key,
                e
            );
            e
        })
    }

    fn get_event(&self, event_offset: u64) -> Result<Option<Vec<u8>>, DatabaseError> {
        self.get_event_at_offset(event_offset).map_err(|e| {
            tracing::error!(
                "Failed to get event from ring buffer '{}': {}",
                self.buffer_key,
                e
            );
            e
        })
    }

    fn load_events(&self) -> Result<Vec<u64>, DatabaseError> {
        let events = self.extract_events_from_buffer();

        info!(
            "Loaded {} events from ring buffer '{}'",
            events.len(),
            self.buffer_key,
        );

        Ok(events)
    }

    async fn clear_storage(&self) -> Result<(), DatabaseError> {
        // Clear in-memory buffer
        {
            let mut buffer = self.buffer.borrow_mut();
            buffer.clear();
        }

        // Reset head offset
        self.head_offset.set(0);

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
