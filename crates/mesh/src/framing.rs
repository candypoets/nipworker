//! Bounded framing for transporting complete Nostr messages over BLE writes.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use crate::MeshError;

const MAGIC: [u8; 2] = *b"NM";
const VERSION: u8 = 1;
const HEADER_LEN: usize = 17;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fragment {
    pub message_id: u32,
    pub index: u16,
    pub count: u16,
    pub total_len: u32,
    pub payload: Vec<u8>,
}

impl Fragment {
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(HEADER_LEN + self.payload.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.push(VERSION);
        bytes.extend_from_slice(&self.message_id.to_be_bytes());
        bytes.extend_from_slice(&self.index.to_be_bytes());
        bytes.extend_from_slice(&self.count.to_be_bytes());
        bytes.extend_from_slice(&self.total_len.to_be_bytes());
        bytes.extend_from_slice(&(self.payload.len() as u16).to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, MeshError> {
        if bytes.len() < HEADER_LEN || bytes[..2] != MAGIC || bytes[2] != VERSION {
            return Err(MeshError::Frame("invalid header".to_string()));
        }
        let message_id = u32::from_be_bytes(bytes[3..7].try_into().unwrap());
        let index = u16::from_be_bytes(bytes[7..9].try_into().unwrap());
        let count = u16::from_be_bytes(bytes[9..11].try_into().unwrap());
        let total_len = u32::from_be_bytes(bytes[11..15].try_into().unwrap());
        let payload_len = u16::from_be_bytes(bytes[15..17].try_into().unwrap()) as usize;
        if count == 0 || index >= count || bytes.len() != HEADER_LEN + payload_len {
            return Err(MeshError::Frame("invalid fragment metadata".to_string()));
        }
        Ok(Self {
            message_id,
            index,
            count,
            total_len,
            payload: bytes[HEADER_LEN..].to_vec(),
        })
    }
}

pub struct Fragmenter {
    next_message_id: u32,
    max_message_size: usize,
}

impl Fragmenter {
    pub fn new(max_message_size: usize) -> Self {
        Self {
            next_message_id: 1,
            max_message_size,
        }
    }

    pub fn fragment(&mut self, message: &[u8], mtu: usize) -> Result<Vec<Vec<u8>>, MeshError> {
        if message.is_empty() || message.len() > self.max_message_size {
            return Err(MeshError::Frame("message size outside limits".to_string()));
        }
        let payload_size = mtu
            .checked_sub(HEADER_LEN)
            .filter(|size| *size > 0)
            .ok_or_else(|| MeshError::Frame("MTU too small".to_string()))?;
        let count = message.len().div_ceil(payload_size);
        if count > u16::MAX as usize {
            return Err(MeshError::Frame("too many fragments".to_string()));
        }
        let message_id = self.next_message_id;
        self.next_message_id = self.next_message_id.wrapping_add(1).max(1);
        Ok(message
            .chunks(payload_size)
            .enumerate()
            .map(|(index, payload)| {
                Fragment {
                    message_id,
                    index: index as u16,
                    count: count as u16,
                    total_len: message.len() as u32,
                    payload: payload.to_vec(),
                }
                .encode()
            })
            .collect())
    }
}

struct Assembly {
    created_at: Instant,
    total_len: usize,
    fragments: Vec<Option<Vec<u8>>>,
    received_bytes: usize,
}

pub struct Reassembler {
    assemblies: HashMap<u32, Assembly>,
    timeout: Duration,
    max_message_size: usize,
    max_inflight_messages: usize,
    inflight_bytes: usize,
    max_inflight_bytes: usize,
}

impl Reassembler {
    pub fn new(
        timeout: Duration,
        max_message_size: usize,
        max_inflight_messages: usize,
        max_inflight_bytes: usize,
    ) -> Self {
        Self {
            assemblies: HashMap::new(),
            timeout,
            max_message_size,
            max_inflight_messages,
            inflight_bytes: 0,
            max_inflight_bytes,
        }
    }

    pub fn push(&mut self, bytes: &[u8], now: Instant) -> Result<Option<Vec<u8>>, MeshError> {
        self.prune(now);
        let fragment = Fragment::decode(bytes)?;
        let total_len = fragment.total_len as usize;
        if total_len == 0 || total_len > self.max_message_size {
            return Err(MeshError::Frame(
                "declared message size outside limits".to_string(),
            ));
        }
        if !self.assemblies.contains_key(&fragment.message_id) {
            if self.assemblies.len() >= self.max_inflight_messages {
                return Err(MeshError::Frame("too many inflight messages".to_string()));
            }
            self.assemblies.insert(
                fragment.message_id,
                Assembly {
                    created_at: now,
                    total_len,
                    fragments: vec![None; fragment.count as usize],
                    received_bytes: 0,
                },
            );
        }
        let assembly = self.assemblies.get_mut(&fragment.message_id).unwrap();
        if assembly.total_len != total_len || assembly.fragments.len() != fragment.count as usize {
            return Err(MeshError::Frame("inconsistent fragment stream".to_string()));
        }
        let slot = &mut assembly.fragments[fragment.index as usize];
        if let Some(existing) = slot {
            if existing != &fragment.payload {
                return Err(MeshError::Frame(
                    "conflicting duplicate fragment".to_string(),
                ));
            }
            return Ok(None);
        }
        if self.inflight_bytes + fragment.payload.len() > self.max_inflight_bytes {
            return Err(MeshError::Frame("inflight byte limit exceeded".to_string()));
        }
        assembly.received_bytes += fragment.payload.len();
        self.inflight_bytes += fragment.payload.len();
        *slot = Some(fragment.payload);

        if assembly.fragments.iter().any(Option::is_none) {
            return Ok(None);
        }
        if assembly.received_bytes != assembly.total_len {
            return Err(MeshError::Frame("reassembled size mismatch".to_string()));
        }
        let assembly = self.assemblies.remove(&fragment.message_id).unwrap();
        self.inflight_bytes -= assembly.received_bytes;
        let mut message = Vec::with_capacity(assembly.total_len);
        for part in assembly.fragments {
            message.extend_from_slice(&part.unwrap());
        }
        Ok(Some(message))
    }

    pub fn prune(&mut self, now: Instant) -> usize {
        let expired: Vec<u32> = self
            .assemblies
            .iter()
            .filter_map(|(id, assembly)| {
                (now.saturating_duration_since(assembly.created_at) >= self.timeout).then_some(*id)
            })
            .collect();
        for id in &expired {
            if let Some(assembly) = self.assemblies.remove(id) {
                self.inflight_bytes -= assembly.received_bytes;
            }
        }
        expired.len()
    }
}

pub struct OutboundQueue {
    frames: VecDeque<Vec<u8>>,
    bytes: usize,
    max_frames: usize,
    max_bytes: usize,
}

impl OutboundQueue {
    pub fn new(max_frames: usize, max_bytes: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            bytes: 0,
            max_frames,
            max_bytes,
        }
    }

    pub fn enqueue(&mut self, frames: Vec<Vec<u8>>) -> Result<(), MeshError> {
        let added_bytes: usize = frames.iter().map(Vec::len).sum();
        if self.frames.len() + frames.len() > self.max_frames
            || self.bytes + added_bytes > self.max_bytes
        {
            return Err(MeshError::Frame("outbound queue full".to_string()));
        }
        self.bytes += added_bytes;
        self.frames.extend(frames);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<Vec<u8>> {
        let frame = self.frames.pop_front()?;
        self.bytes -= frame.len();
        Some(frame)
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragments_and_reassembles_out_of_order() {
        let message = vec![0x42; 10_000];
        let mut fragmenter = Fragmenter::new(16_384);
        let mut frames = fragmenter.fragment(&message, 185).unwrap();
        assert!(frames.len() > 50);
        frames.reverse();
        let now = Instant::now();
        let mut reassembler = Reassembler::new(Duration::from_secs(30), 16_384, 4, 32_768);
        let mut completed = None;
        for frame in frames {
            if let Some(value) = reassembler.push(&frame, now).unwrap() {
                completed = Some(value);
            }
        }
        assert_eq!(completed, Some(message));
    }

    #[test]
    fn identical_duplicate_fragment_is_ignored() {
        let mut fragmenter = Fragmenter::new(1024);
        let frames = fragmenter.fragment(&vec![1; 300], 100).unwrap();
        let now = Instant::now();
        let mut reassembler = Reassembler::new(Duration::from_secs(30), 1024, 4, 4096);
        assert!(reassembler.push(&frames[0], now).unwrap().is_none());
        assert!(reassembler.push(&frames[0], now).unwrap().is_none());
    }

    #[test]
    fn conflicting_duplicate_fragment_is_rejected() {
        let mut fragmenter = Fragmenter::new(1024);
        let frames = fragmenter.fragment(&vec![1; 300], 100).unwrap();
        let mut conflicting = Fragment::decode(&frames[0]).unwrap();
        conflicting.payload[0] ^= 1;
        let now = Instant::now();
        let mut reassembler = Reassembler::new(Duration::from_secs(30), 1024, 4, 4096);
        reassembler.push(&frames[0], now).unwrap();
        assert!(reassembler.push(&conflicting.encode(), now).is_err());
    }

    #[test]
    fn stale_assemblies_are_pruned_and_release_capacity() {
        let mut fragmenter = Fragmenter::new(1024);
        let frames = fragmenter.fragment(&vec![1; 300], 100).unwrap();
        let start = Instant::now();
        let mut reassembler = Reassembler::new(Duration::from_secs(5), 1024, 1, 4096);
        reassembler.push(&frames[0], start).unwrap();
        assert_eq!(reassembler.prune(start + Duration::from_secs(6)), 1);
        assert!(reassembler
            .push(&frames[0], start + Duration::from_secs(6))
            .is_ok());
    }

    #[test]
    fn rejects_oversized_messages_and_tiny_mtu() {
        let mut fragmenter = Fragmenter::new(100);
        assert!(fragmenter.fragment(&vec![0; 101], 185).is_err());
        assert!(fragmenter.fragment(&[1], HEADER_LEN).is_err());
    }

    #[test]
    fn outbound_queue_applies_backpressure_atomically() {
        let mut queue = OutboundQueue::new(2, 10);
        queue.enqueue(vec![vec![1; 4], vec![2; 4]]).unwrap();
        assert!(queue.enqueue(vec![vec![3]]).is_err());
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.pop(), Some(vec![1; 4]));
        assert_eq!(queue.pop(), Some(vec![2; 4]));
        assert!(queue.is_empty());
    }
}
