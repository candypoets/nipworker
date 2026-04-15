//! BatchBuffer stub for the platform-agnostic core crate.
//!
//! In the WASM wrapper, this uses MessagePort.postMessage with transferable
//! ArrayBuffers. In core it is a no-op.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use tracing::{debug, warn};

thread_local! {
    static GLOBAL_BATCH_MANAGER: RefCell<Option<BatchBufferManager>> = RefCell::new(None);
}

pub fn init_global_batch_manager(_port: ()) {
    GLOBAL_BATCH_MANAGER.with(|manager| {
        *manager.borrow_mut() = Some(BatchBufferManager::new());
    });
}

fn with_global_batch_manager<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&BatchBufferManager) -> R,
{
    GLOBAL_BATCH_MANAGER.with(|manager| manager.borrow().as_ref().map(f))
}

fn with_global_batch_manager_mut<F, R>(f: F) -> Option<R>
where
    F: FnOnce(&mut BatchBufferManager) -> R,
{
    GLOBAL_BATCH_MANAGER.with(|manager| manager.borrow_mut().as_mut().map(f))
}

pub fn add_message_to_batch(_sub_id: &str, _data: &[u8]) {
    // no-op in core
}

pub fn flush_all_batches() {
    with_global_batch_manager_mut(|manager| {
        manager.flush_all();
    });
}

pub fn create_batch_buffer(_sub_id: &str) {
    // no-op in core
}

pub fn remove_batch_buffer(_sub_id: &str) {
    // no-op in core
}

pub fn flush_batch(sub_id: &str) {
    with_global_batch_manager_mut(|manager| {
        manager.flush_sub(sub_id);
    });
}

const BATCH_SIZE_THRESHOLD: usize = 16 * 1024;
const BATCH_TIMEOUT_MS: u32 = 50;

pub struct BatchBuffer {
    sub_id: String,
    buffer: Vec<u8>,
    first_event_time: RefCell<Option<u128>>,
}

impl BatchBuffer {
    pub fn new(sub_id: String) -> Self {
        Self {
            sub_id,
            buffer: Vec::with_capacity(BATCH_SIZE_THRESHOLD),
            first_event_time: RefCell::new(None),
        }
    }

    pub fn add_message(&mut self, data: &[u8]) -> bool {
        if self.buffer.is_empty() {
            *self.first_event_time.borrow_mut() =
                Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
        }

        let required_space = 4 + data.len();
        let should_flush_before =
            !self.buffer.is_empty() && (self.buffer.len() + required_space > BATCH_SIZE_THRESHOLD);

        if should_flush_before {
            self.flush();
            *self.first_event_time.borrow_mut() =
                Some(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis());
        }

        let len = data.len() as u32;
        self.buffer.extend_from_slice(&len.to_le_bytes());
        self.buffer.extend_from_slice(data);

        let should_flush_after = self.buffer.len() >= BATCH_SIZE_THRESHOLD;
        if should_flush_after {
            self.flush();
            return true;
        }
        false
    }

    pub fn check_timeout(&self) -> bool {
        if self.buffer.is_empty() {
            return false;
        }
        if let Some(first_time) = *self.first_event_time.borrow() {
            let elapsed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
                - first_time;
            if elapsed >= BATCH_TIMEOUT_MS as u128 {
                return true;
            }
        }
        false
    }

    pub fn flush(&mut self) {
        if self.buffer.is_empty() {
            return;
        }
        debug!("Flushed batch for sub {}: {} bytes", self.sub_id, self.buffer.len());
        self.buffer.clear();
        *self.first_event_time.borrow_mut() = None;
    }

    pub fn len(&self) -> usize {
        self.buffer.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

pub struct BatchBufferManager {
    buffers: RefCell<HashMap<String, Rc<RefCell<BatchBuffer>>>>,
}

impl BatchBufferManager {
    pub fn new() -> Self {
        Self {
            buffers: RefCell::new(HashMap::new()),
        }
    }

    pub fn get_or_create(&self, sub_id: &str) -> Rc<RefCell<BatchBuffer>> {
        let mut buffers = self.buffers.borrow_mut();
        buffers
            .entry(sub_id.to_string())
            .or_insert_with(|| Rc::new(RefCell::new(BatchBuffer::new(sub_id.to_string()))))
            .clone()
    }

    pub fn create_buffer_for_sub(&mut self, sub_id: &str) {
        let mut buffers = self.buffers.borrow_mut();
        if !buffers.contains_key(sub_id) {
            buffers.insert(
                sub_id.to_string(),
                Rc::new(RefCell::new(BatchBuffer::new(sub_id.to_string()))),
            );
        }
    }

    pub fn add_message(&mut self, sub_id: &str, data: &[u8]) {
        if !self.buffers.borrow().contains_key(sub_id) {
            self.create_buffer_for_sub(sub_id);
        }
        if let Some(buffer) = self.buffers.borrow().get(sub_id) {
            buffer.borrow_mut().add_message(data);
        }
    }

    pub fn flush_sub(&self, sub_id: &str) {
        if let Some(buffer) = self.buffers.borrow().get(sub_id) {
            buffer.borrow_mut().flush();
        }
    }

    pub fn flush_all(&self) {
        for (_, buffer) in self.buffers.borrow().iter() {
            buffer.borrow_mut().flush();
        }
    }

    pub fn check_timeouts(&self) {
        for (_, buffer) in self.buffers.borrow().iter() {
            if buffer.borrow().check_timeout() {
                buffer.borrow_mut().flush();
            }
        }
    }

    pub fn remove(&self, sub_id: &str) {
        self.flush_sub(sub_id);
        self.buffers.borrow_mut().remove(sub_id);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_message_format() {
        let test_data = vec![1u8, 2, 3, 4, 5];
        let len = test_data.len() as u32;
        let len_bytes = len.to_le_bytes();
        assert_eq!(len_bytes.len(), 4);
        assert_eq!(u32::from_le_bytes(len_bytes), len);
    }
}
