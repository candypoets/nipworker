use std::cell::RefCell;

use js_sys::{SharedArrayBuffer, Uint8Array};
use wasm_bindgen::prelude::*;

/// Layout constants (little-endian)
/// Header (32 bytes total):
/// 0x00..0x04: capacity (u32)         -> size of data region in bytes
/// 0x04..0x08: head (u32)             -> write index (0..capacity-1)
/// 0x08..0x0C: tail (u32)             -> read index  (0..capacity-1)
/// 0x0C..0x10: seq (u32)              -> monotonically increasing write seq
/// 0x10..0x20: reserved (16 bytes)
///
/// Data region (capacity bytes) immediately follows the header.
///
/// Record framing within the data region (variable length):
///   [ len:u32 | type:u16 | pad:u16 | seq:u32 | payload:[N bytes] | len_trailer:u32 ]
/// where len = 8 + N (the size of [type|pad|seq|payload]) and len_trailer == len.
///
/// Commit safety (no Atomics required):
/// The writer commits a record by writing len_trailer last. The reader treats a record
/// as fully committed only when len_trailer == len.
///
/// Overwrite-on-full:
/// If there is not enough space to write a record, the writer advances the tail forward
/// (skipping fully committed records) to make space (dropping oldest records). If space
/// still cannot be made (because the next record is not yet committed), the writer drops
/// the new record and returns false.
const HEADER_SIZE: usize = 32;

#[inline]
fn data_start() -> usize {
    HEADER_SIZE
}

/// A SharedArrayBuffer ring accessed from Rust (wasm).
/// This implementation copies to/from the SAB using Uint8Array views.
/// It works in single-producer/single-consumer scenarios without Atomics.
pub struct SabRing {
    sab: SharedArrayBuffer,
    view: Uint8Array, // entire SAB
    capacity: usize,  // size in bytes of the data region (after header)
}

impl SabRing {
    /// Create a ring over a SharedArrayBuffer. The SAB length must be HEADER_SIZE + capacity.
    /// If the header is uninitialized, call `initialize_header()` once to set it up.
    pub fn new(sab: SharedArrayBuffer) -> Result<Self, JsValue> {
        let view = Uint8Array::new(&sab);
        let total_len = view.length() as usize;

        if total_len < HEADER_SIZE {
            return Err(JsValue::from_str(&format!(
                "SAB too small for header: length {} < {}",
                total_len, HEADER_SIZE
            )));
        }

        // Try to read capacity from header; if zero, use (total_len - HEADER_SIZE) as capacity.
        let mut cap = {
            let mut tmp = [0u8; 4];
            view.subarray(0, 4).copy_to(&mut tmp);
            u32::from_le_bytes(tmp) as usize
        };

        if cap == 0 {
            cap = total_len.saturating_sub(HEADER_SIZE);
        }

        if HEADER_SIZE + cap != total_len {
            return Err(JsValue::from_str(&format!(
                "SAB length {} != header + capacity {}",
                total_len,
                HEADER_SIZE + cap
            )));
        }

        Ok(Self {
            sab,
            view,
            capacity: cap,
        })
    }

    /// Initialize the ring header (capacity, head=0, tail=0, seq=0).
    /// Only call this once on a fresh SAB, or when you intentionally reinitialize the ring.
    pub fn initialize_header(&mut self) -> Result<(), JsValue> {
        let cap_u32 = (self.capacity as u32).to_le_bytes();
        self.write_header_bytes(0, &cap_u32)?;
        self.write_header_bytes(4, &0u32.to_le_bytes())?;
        self.write_header_bytes(8, &0u32.to_le_bytes())?;
        self.write_header_bytes(12, &0u32.to_le_bytes())?;
        // Zero reserved
        let zero16 = [0u8; 16];
        self.write_header_bytes(16, &zero16)?;
        Ok(())
    }

    /// Returns true if the ring has any records (tail != head).
    pub fn has_records(&self) -> bool {
        self.head() != self.tail()
    }

    /// Read next committed record payload as Vec<u8>. Returns None if empty or uncommitted.
    /// Advances the tail on successful read.
    pub fn read_next(&mut self) -> Option<Vec<u8>> {
        let tail = self.tail();
        let head = self.head();
        if tail == head {
            return None; // empty
        }

        // len at [tail]
        let len = self.ring_read_u32(tail) as usize;
        if len == 0 {
            return None;
        }

        // trailer at tail + 4 + len
        let trailer_pos = (tail + 4 + len) % self.capacity;
        let trailer = self.ring_read_u32(trailer_pos) as usize;
        if trailer != len {
            // Not committed yet
            return None;
        }

        // variable segment starts after len: [type:u16|pad:u16|seq:u32|payload:N]
        // skip 8 bytes -> payload_pos
        let payload_len = len - 8;
        let payload_pos = (tail + 4 + 8) % self.capacity;

        let mut out = vec![0u8; payload_len];
        self.ring_read(payload_pos, &mut out, payload_len);

        // advance tail past len + var len + trailer (4 + len + 4)
        let new_tail = (tail + 4 + len + 4) % self.capacity;
        self.set_tail(new_tail);

        Some(out)
    }

    /// Write a payload as a record. Overwrites oldest records if necessary.
    /// Returns true if the record was successfully written and committed, false if dropped.
    pub fn write(&mut self, payload: &[u8]) -> bool {
        let n = payload.len();
        // variable part after len: type:u16 + pad:u16 + seq:u32 + payload (8 + n)
        let var_len = 8 + n;
        let total = 4 + var_len + 4; // len + var + trailer

        if total > self.capacity {
            // Too large for this ring regardless of overwrite policy
            return false;
        }

        // Overwrite-on-full: drop oldest committed records until enough space
        self.make_space(total);
        if self.free() < total {
            // Still not enough (maybe next record not committed yet)
            return false;
        }

        let head = self.head();
        let seq = self.seq().wrapping_add(1);

        // 1) write len
        self.ring_write_u32(head, var_len as u32);

        // 2) write type=0, pad=0, seq
        let var_pos = (head + 4) % self.capacity;
        self.ring_write(var_pos, &0u16.to_le_bytes());
        self.ring_write((var_pos + 2) % self.capacity, &0u16.to_le_bytes());
        self.ring_write((var_pos + 4) % self.capacity, &seq.to_le_bytes());

        // 3) write payload
        self.ring_write((var_pos + 8) % self.capacity, payload);

        // 4) write trailer (len) last to commit
        let trailer_pos = (head + 4 + var_len) % self.capacity;
        self.ring_write_u32(trailer_pos, var_len as u32);

        // 5) advance head, bump seq
        let new_head = (head + total) % self.capacity;
        self.set_head(new_head);
        self.set_seq(seq);

        true
    }

    #[inline]
    fn head(&self) -> usize {
        self.read_header_u32(4) as usize % self.capacity
    }

    #[inline]
    fn set_head(&mut self, head: usize) {
        self.write_header_bytes(4, &((head % self.capacity) as u32).to_le_bytes())
            .expect("write head failed");
    }

    #[inline]
    fn tail(&self) -> usize {
        self.read_header_u32(8) as usize % self.capacity
    }

    #[inline]
    fn set_tail(&mut self, tail: usize) {
        self.write_header_bytes(8, &((tail % self.capacity) as u32).to_le_bytes())
            .expect("write tail failed");
    }

    #[inline]
    fn seq(&self) -> u32 {
        self.read_header_u32(12)
    }

    #[inline]
    fn set_seq(&mut self, seq: u32) {
        self.write_header_bytes(12, &seq.to_le_bytes())
            .expect("write seq failed");
    }

    #[inline]
    fn used(&self) -> usize {
        let head = self.head();
        let tail = self.tail();
        (head + self.capacity - tail) % self.capacity
    }

    #[inline]
    fn free(&self) -> usize {
        self.capacity - self.used()
    }

    /// Make space by skipping fully-committed records. Overwrite-on-full policy.
    fn make_space(&mut self, needed: usize) {
        while self.free() < needed && self.has_records() {
            if !self.skip_record() {
                // Next record not committed yet; cannot skip further
                break;
            }
        }
    }

    /// Skip one fully-committed record; returns true if skipped.
    fn skip_record(&mut self) -> bool {
        let tail = self.tail();
        if tail == self.head() {
            return false; // empty
        }
        let len = self.ring_read_u32(tail) as usize;
        if len == 0 {
            return false;
        }
        let trailer_pos = (tail + 4 + len) % self.capacity;
        let trailer = self.ring_read_u32(trailer_pos) as usize;
        if trailer != len {
            return false; // not committed yet
        }

        let new_tail = (tail + 4 + len + 4) % self.capacity;
        self.set_tail(new_tail);
        true
    }

    // -------- Header R/W (Uint8Array set/copy) --------

    fn read_header_u32(&self, offset: usize) -> u32 {
        let mut tmp = [0u8; 4];
        self.view
            .subarray(offset as u32, (offset + 4) as u32)
            .copy_to(&mut tmp);
        u32::from_le_bytes(tmp)
    }

    fn write_header_bytes(&mut self, offset: usize, bytes: &[u8]) -> Result<(), JsValue> {
        let tmp = Uint8Array::new_with_length(bytes.len() as u32);
        tmp.copy_from(bytes);
        // Set directly into SAB at header offset
        self.view.set(&tmp, offset as u32);
        Ok(())
    }

    // -------- Data region R/W with wrap-around --------

    fn ring_abs(&self, pos: usize) -> usize {
        data_start() + (pos % self.capacity)
    }

    fn ring_read(&self, mut pos: usize, out: &mut [u8], mut len: usize) {
        let mut out_off = 0usize;
        while len > 0 {
            let to_end = self.capacity - (pos % self.capacity);
            let chunk = len.min(to_end);
            let abs = self.ring_abs(pos);
            self.view
                .subarray(abs as u32, (abs + chunk) as u32)
                .copy_to(&mut out[out_off..out_off + chunk]);
            len -= chunk;
            out_off += chunk;
            pos = (pos + chunk) % self.capacity;
        }
    }

    fn ring_write(&mut self, mut pos: usize, src: &[u8]) {
        let mut remaining = src.len();
        let mut src_off = 0usize;
        while remaining > 0 {
            let to_end = self.capacity - (pos % self.capacity);
            let chunk = remaining.min(to_end);
            let abs = self.ring_abs(pos);

            // Prepare a temporary JS typed array and copy from Rust slice
            let tmp = Uint8Array::new_with_length(chunk as u32);
            tmp.copy_from(&src[src_off..src_off + chunk]);

            // Set into SAB at the absolute offset
            self.view.set(&tmp, abs as u32);

            remaining -= chunk;
            src_off += chunk;
            pos = (pos + chunk) % self.capacity;
        }
    }

    fn ring_read_u32(&self, pos: usize) -> u32 {
        let mut tmp = [0u8; 4];
        self.ring_read(pos, &mut tmp, 4);
        u32::from_le_bytes(tmp)
    }

    fn ring_write_u32(&mut self, pos: usize, v: u32) {
        self.ring_write(pos, &v.to_le_bytes());
    }
}

/// Helper that pairs the two rings used by the WS worker:
/// - in_ring: Rust → TS (JSON envelopes to send to relays)
/// - out_ring: TS → Rust (WorkerLine bytes from relays)
pub struct WsRings {
    pub in_ring: RefCell<SabRing>,
    pub out_ring: RefCell<SabRing>,
}

impl WsRings {
    /// Construct from two SharedArrayBuffers. The buffers should already be allocated
    /// in JS with the full size (HEADER_SIZE + desired capacity).
    pub fn new(in_sab: SharedArrayBuffer, out_sab: SharedArrayBuffer) -> Result<Self, JsValue> {
        let in_ring = SabRing::new(in_sab)?;
        let out_ring = SabRing::new(out_sab)?;
        Ok(Self {
            in_ring: RefCell::new(in_ring),
            out_ring: RefCell::new(out_ring),
        })
    }

    /// Initialize both rings' headers (capacity, head, tail, seq). Only call this
    /// when you intentionally reinitialize the rings.
    pub fn initialize(&self) -> Result<(), JsValue> {
        self.in_ring.borrow_mut().initialize_header()?;
        self.out_ring.borrow_mut().initialize_header()?;
        Ok(())
    }

    /// Read one message from out_ring (TS → Rust). Returns the payload bytes
    /// (e.g., a FlatBuffers-serialized WorkerLine).
    pub fn read_out(&self) -> Option<Vec<u8>> {
        self.out_ring.borrow_mut().read_next()
    }
}
