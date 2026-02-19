//! BatchBuffer for accumulating and sending batched events via MessageChannel.
//!
//! This module provides efficient batched event delivery from the parser worker
//! to the main thread using MessagePort.postMessage with transferable ArrayBuffers.
//!
//! Batching criteria:
//! - Buffer size exceeds 16KB, OR
//! - Timeout exceeds 50ms since first event in batch
//!
//! Data format: [4-byte len (little endian)][WorkerMessage][4-byte len][WorkerMessage]...
//! (Same format as SharedArrayBuffer for compatibility with ArrayBufferReader)

use js_sys::{Array, Object, Reflect, Uint8Array};
use std::cell::RefCell;
use std::rc::Rc;
use tracing::{debug, warn};
use wasm_bindgen::JsValue;
use web_sys::MessagePort;

// Thread-local storage for the global BatchBufferManager instance
thread_local! {
    static GLOBAL_BATCH_MANAGER: RefCell<Option<BatchBufferManager>> = RefCell::new(None);
}

/// Initialize the global BatchBufferManager singleton.
/// This should be called once during NetworkManager initialization.
pub fn init_global_batch_manager(port: MessagePort) {
    GLOBAL_BATCH_MANAGER.with(|manager| {
        *manager.borrow_mut() = Some(BatchBufferManager::new(port));
    });
}

/// Get a reference to the global BatchBufferManager if initialized.
fn with_global_batch_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&BatchBufferManager) -> R,
{
    GLOBAL_BATCH_MANAGER.with(|manager| {
        manager.borrow().as_ref().map(f)
    })
}

/// Get a mutable reference to the global BatchBufferManager if initialized.
fn with_global_batch_manager_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut BatchBufferManager) -> R,
{
    GLOBAL_BATCH_MANAGER.with(|manager| {
        manager.borrow_mut().as_mut().map(f)
    })
}

/// Add a message to a subscription's batch buffer via the global manager.
/// This is a convenience function that can be called from anywhere.
pub fn add_message_to_batch(sub_id: &str, data: &[u8]) {
    with_global_batch_manager_mut(|manager| {
        manager.add_message(sub_id, data);
    });
}

/// Flush all pending batches via the global manager.
pub fn flush_all_batches() {
    with_global_batch_manager_mut(|manager| {
        manager.flush_all();
    });
}

/// Create a batch buffer for a subscription via the global manager.
pub fn create_batch_buffer(sub_id: &str) {
    with_global_batch_manager_mut(|manager| {
        manager.create_buffer_for_sub(sub_id);
    });
}

/// Remove a subscription's batch buffer via the global manager.
pub fn remove_batch_buffer(sub_id: &str) {
    with_global_batch_manager_mut(|manager| {
        manager.remove(sub_id);
    });
}

/// Default batch size threshold: 16KB
const BATCH_SIZE_THRESHOLD: usize = 16 * 1024;
/// Default timeout threshold: 50ms
const BATCH_TIMEOUT_MS: u32 = 50;

/// BatchBuffer accumulates events and sends them via MessagePort when thresholds are reached.
pub struct BatchBuffer {
    /// The subscription ID this buffer belongs to
    sub_id: String,
    /// Accumulated data buffer
    buffer: Vec<u8>,
    /// Timestamp when the first event was added to current batch (for timeout tracking)
    first_event_time: RefCell<Option<f64>>,
    /// The MessagePort to send batched data through
    port: MessagePort,
}

impl BatchBuffer {
    /// Create a new BatchBuffer for a subscription
    pub fn new(sub_id: String, port: MessagePort) -> Self {
        Self {
            sub_id,
            buffer: Vec::with_capacity(BATCH_SIZE_THRESHOLD),
            first_event_time: RefCell::new(None),
            port,
        }
    }

    /// Add a serialized WorkerMessage to the batch.
    /// Format: [4-byte len (little endian)][WorkerMessage bytes]
    /// Returns true if the batch was flushed, false otherwise.
    pub fn add_message(&mut self, data: &[u8]) -> bool {
        // Check if this is the first event in the batch
        if self.buffer.is_empty() {
            *self.first_event_time.borrow_mut() = Some(js_sys::Date::now());
        }

        // Calculate required space: 4 bytes for length prefix + data length
        let required_space = 4 + data.len();

        // Check if adding this message would exceed the threshold
        // (only flush if we already have data and this would push us over)
        let should_flush_before = !self.buffer.is_empty()
            && (self.buffer.len() + required_space > BATCH_SIZE_THRESHOLD);

        if should_flush_before {
            self.flush();
            *self.first_event_time.borrow_mut() = Some(js_sys::Date::now());
        }

        // Write length prefix (4 bytes, little endian)
        let len = data.len() as u32;
        self.buffer.extend_from_slice(&len.to_le_bytes());

        // Write the actual data
        self.buffer.extend_from_slice(data);

        // Check if we've now exceeded the threshold after adding
        let should_flush_after = self.buffer.len() >= BATCH_SIZE_THRESHOLD;

        if should_flush_after {
            self.flush();
            return true;
        }

        false
    }

    /// Check if the batch should be flushed due to timeout.
    /// Returns true if flushed, false otherwise.
    pub fn check_timeout(&self) -> bool {
        if self.buffer.is_empty() {
            return false;
        }

        if let Some(first_time) = *self.first_event_time.borrow() {
            let elapsed = js_sys::Date::now() - first_time;
            if elapsed >= BATCH_TIMEOUT_MS as f64 {
                return true;
            }
        }

        false
    }

    /// Flush the current batch via MessagePort.postMessage with transferable ArrayBuffer.
    /// This sends { subId, data } where data is a Uint8Array (backed by transferable ArrayBuffer).
    pub fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }

        // Create a Uint8Array from our buffer data
        let uint8_array = Uint8Array::new_with_length(self.buffer.len() as u32);
        uint8_array.copy_from(&self.buffer);

        // Create the message object: { subId, data }
        let message = match Self::create_message_object(&self.sub_id, &uint8_array) {
            Ok(msg) => msg,
            Err(e) => {
                warn!("Failed to create message object for sub {}: {:?}", self.sub_id, e);
                // Clear buffer even on error to avoid infinite growth
                self.buffer.clear();
                *self.first_event_time.borrow_mut() = None;
                return;
            }
        };

        // Create transferables array with the Uint8Array's buffer
        let transferables = Array::new();
        transferables.push(&uint8_array.buffer());

        // Send via MessagePort with transferable
        match self.port.post_message_with_transferable(&message, &transferables) {
            Ok(_) => {
                debug!(
                    "Flushed batch for sub {}: {} bytes",
                    self.sub_id,
                    self.buffer.len()
                );
            }
            Err(e) => {
                warn!("Failed to send batch for sub {}: {:?}", self.sub_id, e);
            }
        }

        // Clear the buffer and reset timer
        self.buffer.clear();
        *self.first_event_time.borrow_mut() = None;
    }

    /// Create a JavaScript object: { subId: string, data: Uint8Array }
    fn create_message_object(sub_id: &str, data: &Uint8Array) -> Result<JsValue, JsValue> {
        let obj = Object::new();
        Reflect::set(&obj, &JsValue::from_str("subId"), &JsValue::from_str(sub_id))?;
        Reflect::set(&obj, &JsValue::from_str("data"), data)?;
        Ok(obj.into())
    }

    /// Get the current buffer size in bytes
    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    /// Check if the buffer is empty
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

/// BatchBufferManager manages BatchBuffers for multiple subscriptions.
/// It handles periodic timeout checking and flushing.
pub struct BatchBufferManager {
    buffers: RefCell<std::collections::HashMap<String, Rc<RefCell<BatchBuffer>>>>,
    port: MessagePort,
}

impl BatchBufferManager {
    /// Create a new BatchBufferManager with a MessagePort
    pub fn new(port: MessagePort) -> Self {
        Self {
            buffers: RefCell::new(std::collections::HashMap::new()),
            port,
        }
    }

    /// Get or create a BatchBuffer for a subscription
    pub fn get_or_create(&self, sub_id: &str) -> Rc<RefCell<BatchBuffer>> {
        let mut buffers = self.buffers.borrow_mut();
        let port = self.port.clone();
        buffers
            .entry(sub_id.to_string())
            .or_insert_with(move || Rc::new(RefCell::new(BatchBuffer::new(sub_id.to_string(), port))))
            .clone()
    }

    /// Create a batch buffer for a specific subscription (used by global functions)
    pub fn create_buffer_for_sub(&mut self, sub_id: &str) {
        let mut buffers = self.buffers.borrow_mut();
        if !buffers.contains_key(sub_id) {
            let port = self.port.clone();
            buffers.insert(
                sub_id.to_string(),
                Rc::new(RefCell::new(BatchBuffer::new(sub_id.to_string(), port))),
            );
        }
    }

    /// Add a message to the appropriate subscription's batch buffer.
    /// The message should already be serialized as a WorkerMessage FlatBuffer.
    /// Format: [4-byte len (little endian)][WorkerMessage bytes]
    pub fn add_message(&mut self, sub_id: &str, data: &[u8]) {
        // Auto-create buffer if it doesn't exist
        if !self.buffers.borrow().contains_key(sub_id) {
            self.create_buffer_for_sub(sub_id);
        }
        if let Some(buffer) = self.buffers.borrow().get(sub_id) {
            buffer.borrow_mut().add_message(data);
        }
    }

    /// Flush a specific subscription's buffer
    pub fn flush_sub(&self, sub_id: &str) {
        if let Some(buffer) = self.buffers.borrow().get(sub_id) {
            buffer.borrow_mut().flush();
        }
    }

    /// Flush all buffers (useful for shutdown or EOSE)
    pub fn flush_all(&self) {
        for (_, buffer) in self.buffers.borrow().iter() {
            buffer.borrow_mut().flush();
        }
    }

    /// Check all buffers for timeout and flush if needed.
    /// This should be called periodically (e.g., every 50ms).
    pub fn check_timeouts(&self) {
        for (_, buffer) in self.buffers.borrow().iter() {
            if buffer.borrow().check_timeout() {
                buffer.borrow_mut().flush();
            }
        }
    }

    /// Remove a subscription's buffer (e.g., when subscription is closed)
    pub fn remove(&self, sub_id: &str) {
        // Flush any remaining data before removing
        self.flush_sub(sub_id);
        self.buffers.borrow_mut().remove(sub_id);
    }
}

// Note: Default is not implemented for BatchBufferManager because it requires a MessagePort
// Use BatchBufferManager::new(port) instead

#[cfg(test)]
mod tests {
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_worker);

    #[wasm_bindgen_test]
    fn test_message_format() {
        // Test that the message format is correct
        let test_data = vec![1u8, 2, 3, 4, 5];
        let len = test_data.len() as u32;
        let len_bytes = len.to_le_bytes();

        assert_eq!(len_bytes.len(), 4);
        assert_eq!(u32::from_le_bytes(len_bytes), len);
    }
}
