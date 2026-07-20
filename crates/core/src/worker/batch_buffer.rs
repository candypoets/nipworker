//! BatchBuffer for accumulating parser→main frames before sending.
//!
//! Live (post-EOSE) events used to be forwarded one channel message per event.
//! This module batches them so a single channel message carries many events,
//! cutting cross-thread postMessage traffic on the parser→main channel.
//!
//! Batching criteria (per subscription):
//! - Buffer size reaches 16KB (flushed synchronously on add), OR
//! - 50ms elapsed since the first buffered frame (flushed by the caller's sweep)
//!
//! Wire format of a flushed payload: concatenated frames of
//!   [4-byte frame len LE][4-byte subIdLen LE][subId][WorkerMessage]
//! i.e. length-prefixed `encode_tagged` frames, decoded by
//! `parser_worker::decode_tagged_batch` and the main-thread ArrayBufferReader.

use crate::platform::now_millis;
use crate::worker::parser_worker::encode_tagged;
use rustc_hash::FxHashMap;

/// Flush once a subscription's buffer reaches 16KB.
pub const BATCH_SIZE_THRESHOLD: usize = 16 * 1024;
/// Flush once the oldest buffered frame is 50ms old.
pub const BATCH_TIMEOUT_MS: u64 = 50;

/// Per-subscription accumulator of length-prefixed tagged frames.
struct BatchBuffer {
    buf: Vec<u8>,
    /// Timestamp (ms) of the first frame in the current batch.
    first_at: u64,
}

impl BatchBuffer {
    fn new() -> Self {
        Self {
            buf: Vec::with_capacity(BATCH_SIZE_THRESHOLD),
            first_at: 0,
        }
    }

    /// Append a frame; returns a payload to send if the size threshold
    /// forced a flush (before and/or after appending).
    fn add_frame(&mut self, frame: &[u8], now: u64) -> Option<Vec<u8>> {
        // Flush the current batch first if this frame would push it over
        // the threshold (keeps frames whole; one payload stays <= ~16KB).
        let mut pending = None;
        if !self.buf.is_empty() && self.buf.len() + frame.len() > BATCH_SIZE_THRESHOLD {
            pending = Some(std::mem::take(&mut self.buf));
        }
        if self.buf.is_empty() {
            self.first_at = now;
        }
        self.buf.extend_from_slice(frame);
        // A single frame larger than the threshold (or an exact landing on it)
        // flushes immediately.
        if self.buf.len() >= BATCH_SIZE_THRESHOLD {
            return Some(std::mem::take(&mut self.buf));
        }
        pending
    }

    fn timed_out(&self, now: u64) -> bool {
        !self.buf.is_empty() && now.saturating_sub(self.first_at) >= BATCH_TIMEOUT_MS
    }

    fn take(&mut self) -> Option<Vec<u8>> {
        if self.buf.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.buf))
        }
    }
}

/// Manages one BatchBuffer per subscription id.
pub struct BatchBufferManager {
    buffers: FxHashMap<String, BatchBuffer>,
}

impl BatchBufferManager {
    pub fn new() -> Self {
        Self {
            buffers: FxHashMap::default(),
        }
    }

    /// Buffer a serialized WorkerMessage for `sub_id`, framed as
    /// `[4B len][encode_tagged(sub_id, data)]`.
    /// Returns a flushed payload when the size threshold forces a flush.
    pub fn add_message(&mut self, sub_id: &str, data: &[u8]) -> Option<Vec<u8>> {
        let tagged = encode_tagged(sub_id, data);
        let mut frame = Vec::with_capacity(4 + tagged.len());
        frame.extend_from_slice(&(tagged.len() as u32).to_le_bytes());
        frame.extend_from_slice(&tagged);
        let now = now_millis();
        self.buffers
            .entry(sub_id.to_string())
            .or_insert_with(BatchBuffer::new)
            .add_frame(&frame, now)
    }

    /// Drain a subscription's buffer; returns the payload if non-empty.
    /// The buffer entry is removed so closed subscriptions don't leak entries.
    pub fn flush_sub(&mut self, sub_id: &str) -> Option<Vec<u8>> {
        let payload = self.buffers.get_mut(sub_id).and_then(BatchBuffer::take);
        if payload.is_some() {
            self.buffers.remove(sub_id);
        }
        payload
    }

    /// Drain every buffer whose oldest frame exceeded the timeout.
    pub fn drain_timed_out(&mut self) -> Vec<Vec<u8>> {
        let now = now_millis();
        let timed_out: Vec<String> = self
            .buffers
            .iter()
            .filter(|(_, b)| b.timed_out(now))
            .map(|(k, _)| k.clone())
            .collect();
        let mut out = Vec::with_capacity(timed_out.len());
        for key in timed_out {
            if let Some(payload) = self.flush_sub(&key) {
                out.push(payload);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::parser_worker::decode_tagged_batch;

    #[test]
    fn buffers_until_explicit_flush() {
        let mut mgr = BatchBufferManager::new();
        assert!(mgr.add_message("s1", b"hello").is_none());
        assert!(mgr.add_message("s1", b"world").is_none());

        // Nothing flushed for other subs
        assert!(mgr.flush_sub("other").is_none());

        let payload = mgr.flush_sub("s1").expect("expected pending batch");
        let frames = decode_tagged_batch(&payload);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0], ("s1".to_string(), b"hello".to_vec()));
        assert_eq!(frames[1], ("s1".to_string(), b"world".to_vec()));

        // Buffer drained and entry removed
        assert!(mgr.flush_sub("s1").is_none());
        assert!(mgr.buffers.is_empty());
    }

    #[test]
    fn flushes_on_size_threshold() {
        let mut mgr = BatchBufferManager::new();
        // Fill most of the buffer with small frames
        let chunk = vec![0u8; 8 * 1024];
        assert!(mgr.add_message("s1", &chunk).is_none());
        // This frame pushes the existing batch over 16KB -> flush-before
        let flushed = mgr.add_message("s1", &chunk).expect("expected size flush");
        let frames = decode_tagged_batch(&flushed);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].1, chunk);
        // The second frame remains buffered
        let rest = mgr.flush_sub("s1").expect("expected remainder");
        assert_eq!(decode_tagged_batch(&rest).len(), 1);
    }

    #[test]
    fn oversized_frame_flushes_immediately() {
        let mut mgr = BatchBufferManager::new();
        let big = vec![1u8; BATCH_SIZE_THRESHOLD];
        let flushed = mgr
            .add_message("s1", &big)
            .expect("oversized frame should flush immediately");
        let frames = decode_tagged_batch(&flushed);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].1, big);
        assert!(mgr.flush_sub("s1").is_none());
    }

    #[test]
    fn timeout_drains_only_stale_buffers() {
        let mut mgr = BatchBufferManager::new();
        assert!(mgr.add_message("s1", b"a").is_none());
        // Not yet timed out
        assert!(mgr.drain_timed_out().is_empty());
        // Force the buffer to look stale by backdating first_at
        if let Some(buf) = mgr.buffers.get_mut("s1") {
            buf.first_at = now_millis().saturating_sub(BATCH_TIMEOUT_MS + 1);
        }
        let drained = mgr.drain_timed_out();
        assert_eq!(drained.len(), 1);
        assert_eq!(decode_tagged_batch(&drained[0]).len(), 1);
        assert!(mgr.buffers.is_empty());
    }

    #[test]
    fn per_subscription_isolation() {
        let mut mgr = BatchBufferManager::new();
        assert!(mgr.add_message("a", b"1").is_none());
        assert!(mgr.add_message("b", b"2").is_none());

        let pa = mgr.flush_sub("a").expect("batch for a");
        let fa = decode_tagged_batch(&pa);
        assert_eq!(fa.len(), 1);
        assert_eq!(fa[0].0, "a");

        let pb = mgr.flush_sub("b").expect("batch for b");
        let fb = decode_tagged_batch(&pb);
        assert_eq!(fb.len(), 1);
        assert_eq!(fb[0].0, "b");
    }
}
