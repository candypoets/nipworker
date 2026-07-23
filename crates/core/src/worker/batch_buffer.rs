//! BatchBuffer for accumulating frames before sending on a worker channel.
//!
//! Live (post-EOSE) events used to be forwarded one channel message per event.
//! This module batches them so a single channel message carries many events,
//! cutting cross-thread postMessage traffic. Used on the parser→main channel
//! and on the connections→parser channel.
//!
//! Batching criteria (per subscription):
//! - Buffer size reaches 16KB (flushed synchronously on add), OR
//! - 8ms elapsed since the first buffered frame (flushed by the caller's sweep)
//!
//! Wire format of a flushed payload: concatenated frames of
//!   [4-byte frame len LE][4-byte subIdLen LE][subId][WorkerMessage]
//! i.e. length-prefixed `encode_tagged` frames, decoded by
//! `decode_tagged_batch` and the main-thread ArrayBufferReader.
//!
//! On the connections→parser channel a flushed payload is additionally
//! wrapped with `CONN_BATCH_MAGIC` (`encode_conn_batch`) so the parser can
//! tell a batch apart from a bare single WorkerMessage (the TS proxy path
//! still sends those).

use crate::platform::now_millis;
use rustc_hash::FxHashMap;
use tracing::warn;

/// Flush once a subscription's buffer reaches 16KB.
pub const BATCH_SIZE_THRESHOLD: usize = 16 * 1024;
/// Flush once the oldest buffered frame is 8ms old.
pub const BATCH_TIMEOUT_MS: u64 = 8;

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

/// Encode a (sub_id, data) pair into a single Vec<u8> using a simple length-prefix format:
/// [4 bytes: sub_id_len LE][sub_id_bytes][data_bytes]
pub fn encode_tagged(sub_id: &str, data: &[u8]) -> Vec<u8> {
    let sub_bytes = sub_id.as_bytes();
    let mut buf = Vec::with_capacity(4 + sub_bytes.len() + data.len());
    buf.extend_from_slice(&(sub_bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(sub_bytes);
    buf.extend_from_slice(data);
    buf
}

/// Decode a tagged byte stream back into (sub_id, data).
/// Returns None if the buffer is too short or malformed.
pub fn decode_tagged(bytes: &[u8]) -> Option<(String, Vec<u8>)> {
    if bytes.len() < 4 {
        return None;
    }
    let sub_len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() < 4 + sub_len {
        return None;
    }
    let sub_id = String::from_utf8_lossy(&bytes[4..4 + sub_len]).to_string();
    let data = bytes[4 + sub_len..].to_vec();
    Some((sub_id, data))
}

/// Decode a batched payload: concatenated frames of
/// `[4-byte frame len LE][encode_tagged(sub_id, data)]`.
/// Malformed trailing bytes stop decoding; frames decoded so far are returned.
pub fn decode_tagged_batch(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut frames = Vec::new();
    let mut offset = 0;

    while offset + 4 <= bytes.len() {
        let len = u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ]) as usize;

        if len == 0 || offset + 4 + len > bytes.len() {
            warn!(
                "Invalid batched frame length: {} at offset {} (payload {} bytes)",
                len,
                offset,
                bytes.len()
            );
            break;
        }

        if let Some(frame) = decode_tagged(&bytes[offset + 4..offset + 4 + len]) {
            frames.push(frame);
        }

        offset += 4 + len;
    }

    frames
}

/// Magic prefix marking a batched connections→parser payload whose frames are
/// WorkerMessage FlatBuffers. A bare FlatBuffers WorkerMessage always starts
/// with a small non-zero root-table uoffset, so 0xFFFFFFFF can never collide
/// with one (the TS proxy path and relay status frames still travel as bare
/// single messages).
pub const CONN_BATCH_MAGIC: [u8; 4] = [0xFF, 0xFF, 0xFF, 0xFF];

/// Magic prefix marking a batched connections→parser payload whose frames are
/// raw Nostr EVENT JSON objects (compact envelope): the connections worker
/// skips the FlatBuffer build entirely and the parser feeds the slice
/// straight into the pipeline's JSON scanners. Control frames (EOSE/CLOSED/
/// OK/AUTH/NOTICE) keep the WorkerMessage envelope and never travel in batches.
pub const CONN_RAW_BATCH_MAGIC: [u8; 4] = [0xFE, 0xFF, 0xFF, 0xFF];

/// A decoded connections→parser batch payload.
pub struct ConnBatch {
    /// True when frames are raw EVENT JSON objects, false when they are
    /// WorkerMessage FlatBuffers.
    pub raw_events: bool,
    /// (sub_id, frame data) pairs in arrival order.
    pub frames: Vec<(String, Vec<u8>)>,
}

/// Wrap a flushed batch payload with a connections→parser batch magic.
fn wrap_conn_batch(magic: [u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&magic);
    out.extend_from_slice(payload);
    out
}

/// Wrap a flushed batch payload of WorkerMessage frames with the
/// connections→parser batch magic.
pub fn encode_conn_batch(payload: &[u8]) -> Vec<u8> {
    wrap_conn_batch(CONN_BATCH_MAGIC, payload)
}

/// Wrap a flushed batch payload of raw EVENT JSON frames with the
/// connections→parser raw-batch magic.
pub fn encode_raw_conn_batch(payload: &[u8]) -> Vec<u8> {
    wrap_conn_batch(CONN_RAW_BATCH_MAGIC, payload)
}

/// If `bytes` is a magic-prefixed connections→parser batch, decode it.
/// Returns None for a bare single WorkerMessage, which callers handle on the
/// legacy path.
pub fn decode_conn_batch(bytes: &[u8]) -> Option<ConnBatch> {
    if bytes.len() < 4 {
        return None;
    }
    let magic: [u8; 4] = bytes[0..4].try_into().unwrap();
    let raw_events = match magic {
        CONN_BATCH_MAGIC => false,
        CONN_RAW_BATCH_MAGIC => true,
        _ => return None,
    };
    Some(ConnBatch {
        raw_events,
        frames: decode_tagged_batch(&bytes[4..]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn conn_batch_roundtrip() {
        let mut mgr = BatchBufferManager::new();
        assert!(mgr.add_message("s1", b"ev1").is_none());
        assert!(mgr.add_message("s1", b"ev2").is_none());
        let payload = mgr.flush_sub("s1").expect("pending batch");

        let wrapped = encode_conn_batch(&payload);
        assert_eq!(&wrapped[0..4], &CONN_BATCH_MAGIC);

        let batch = decode_conn_batch(&wrapped).expect("should decode as conn batch");
        assert!(!batch.raw_events);
        assert_eq!(batch.frames.len(), 2);
        assert_eq!(batch.frames[0], ("s1".to_string(), b"ev1".to_vec()));
        assert_eq!(batch.frames[1], ("s1".to_string(), b"ev2".to_vec()));
    }

    #[test]
    fn raw_conn_batch_roundtrip() {
        let mut mgr = BatchBufferManager::new();
        assert!(mgr.add_message("s1", br#"{"id":"a"}"#).is_none());
        let payload = mgr.flush_sub("s1").expect("pending batch");

        let wrapped = encode_raw_conn_batch(&payload);
        assert_eq!(&wrapped[0..4], &CONN_RAW_BATCH_MAGIC);

        let batch = decode_conn_batch(&wrapped).expect("should decode as raw conn batch");
        assert!(batch.raw_events);
        assert_eq!(batch.frames.len(), 1);
        assert_eq!(batch.frames[0].0, "s1");
        assert_eq!(batch.frames[0].1, br#"{"id":"a"}"#.to_vec());
    }

    #[test]
    fn bare_worker_message_is_not_a_conn_batch() {
        // A bare FlatBuffers WorkerMessage starts with a small non-zero
        // root-table uoffset, never with 0xFFFFFFFF.
        let bare = vec![0x0C, 0x00, 0x00, 0x00, 1, 2, 3];
        assert!(decode_conn_batch(&bare).is_none());
        // Too short to carry the magic at all.
        assert!(decode_conn_batch(&[0xFF, 0xFF]).is_none());
    }
}
