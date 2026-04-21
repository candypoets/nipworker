use crate::storage::db::types::{DatabaseConfig, DatabaseError, EventStorage};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tracing::info;

/// Simple ring buffer storage implementation (platform-agnostic, in-memory).
/// IndexedDB persistence has been stubbed out for the core crate refactor.
#[derive(Debug, Clone)]
pub struct RingBufferStorage {
    db_name: String,
    buffer_key: String,
    max_buffer_size: usize,
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
            config,
            buffer: Rc::new(RefCell::new(Vec::with_capacity(max_buffer_size))),
            initialized: Cell::new(false),
            head_offset: Cell::new(0),
        }
    }

    /// Get the database name
    pub fn db_name(&self) -> &str {
        &self.db_name
    }

    /// Get the buffer key
    pub fn buffer_key(&self) -> &str {
        &self.buffer_key
    }

    /// Initialize by loading existing buffer from persistent storage
    pub async fn initialize(&self) -> Result<(), DatabaseError> {
        if self.initialized.get() {
            return Ok(());
        }

        // Persistent storage load is stubbed in the platform-agnostic core.
        // The WASM wrapper will re-implement IndexedDB loading here.
        info!("Initialized ring buffer '{}' (in-memory)", self.buffer_key);
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

        // Persistence is stubbed in the platform-agnostic core.
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

    /// Save the current buffer contents to a byte vector.
    /// Returns a copy of the raw buffer bytes (length-prefixed events).
    pub fn save_to_bytes(&self) -> Vec<u8> {
        self.buffer.borrow().clone()
    }

    /// Load the buffer from a byte vector, replacing all current contents.
    /// The bytes should be length-prefixed events in the same format.
    pub fn load_from_bytes(&self, bytes: &[u8]) -> Result<(), String> {
        // Validate the bytes contain valid length-prefixed events
        let mut p = 0usize;
        let mut event_count = 0usize;
        while p + 4 <= bytes.len() {
            let size = u32::from_le_bytes([bytes[p], bytes[p + 1], bytes[p + 2], bytes[p + 3]])
                as usize;
            if size == 0 {
                return Err(format!("Invalid event size 0 at position {}", p));
            }
            if p + 4 + size > bytes.len() {
                return Err(format!(
                    "Incomplete event at position {}: expected {} bytes, got {}",
                    p,
                    4 + size,
                    bytes.len() - p
                ));
            }
            p += 4 + size;
            event_count += 1;
        }

        if p != bytes.len() {
            return Err(format!(
                "Trailing bytes at position {}: {} bytes remaining",
                p,
                bytes.len() - p
            ));
        }

        // Replace buffer contents
        {
            let mut buffer = self.buffer.borrow_mut();
            buffer.clear();
            buffer.extend_from_slice(bytes);
        }

        // Reset head offset (this is a full replacement, not an append)
        self.head_offset.set(0);

        info!(
            "Loaded {} bytes into ring buffer '{}' ({} events)",
            bytes.len(),
            self.buffer_key,
            event_count
        );

        Ok(())
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

        info!("Cleared ring buffer '{}'", self.buffer_key);
        Ok(())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}
