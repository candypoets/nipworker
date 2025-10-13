//! Utilities to read from the WebSocket worker "out ring" and write to the
//! worker "in ring", using a simple SPSC overwrite-on-full ring buffer layout.
//!
//! Memory layout (little-endian):
//! - Header (32 bytes total):
//!   0x00..0x04: capacity (u32)         -> size of data region in bytes
//!   0x04..0x08: head (u32)             -> write index (0..capacity-1)
//!   0x08..0x0C: tail (u32)             -> read index  (0..capacity-1)
//!   0x0C..0x10: seq (u32)              -> monotonically increasing write seq (optional debug)
//!   0x10..0x20: reserved (16 bytes)
//! - Data region (capacity bytes) immediately follows the header.
//!
//! Record framing within the data region (variable length):
//!   [ len:u32 | type:u16 | pad:u16 | seq:u32 | payload:[N bytes] | len_trailer:u32 ]
//! where len = 8 + N (the size of [type|pad|seq|payload]) and len_trailer == len.
//!
//! Safety against torn reads (no Atomics):
//! The writer commits a record by writing len_trailer last. The reader treats a
//! record as fully committed only when len_trailer == len.
//!
//! Overwrite-on-full:
//! If there is not enough space to write a record, the writer advances the
//! tail forward (skipping fully committed records) to make space (dropping
//! oldest records). If space still cannot be made (because the next record is
//! not yet committed), the writer drops the new record and returns false.
//!
//! This module provides:
//! - `SharedRing`: a ring view over a mutable byte slice (e.g., mapped SAB)
//! - `WsRingBridge`: helper to connect an in-ring writer with an out-ring reader
//! - `parse_root`: helper to parse FlatBuffers roots from read payloads
//! - `write_envelope_json`: helper to send the inbound JSON envelope
//!
//! Note: This is a pure byte-level utility; it does not depend on a specific
//! generated FlatBuffers type. Use `parse_root::<YourFbType>(bytes)` to parse.

use std::io::{self};

use flatbuffers::{self, Follow};

const HEADER_SIZE: usize = 32;

#[inline]
fn data_start() -> usize {
    HEADER_SIZE
}

#[derive(Debug)]
pub struct SharedRing<'a> {
    /// The entire buffer (header + data).
    buf: &'a mut [u8],
    /// Cached capacity (in bytes) of the data region.
    capacity: usize,
}

impl<'a> SharedRing<'a> {
    /// Create a view over the provided buffer. The buffer must contain the full
    /// header + data region layout, and the header must have been initialized
    /// (at least the capacity field).
    pub fn new(buf: &'a mut [u8]) -> io::Result<Self> {
        if buf.len() < HEADER_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "buffer smaller than header",
            ));
        }
        let capacity = u32::from_le_bytes(buf[0..4].try_into().unwrap()) as usize;
        if buf.len() != HEADER_SIZE + capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "buffer length {} does not match header capacity {} + header {}",
                    buf.len(),
                    capacity,
                    HEADER_SIZE
                ),
            ));
        }
        Ok(Self { buf, capacity })
    }

    /// Initialize an empty ring header given a buffer sized as (HEADER_SIZE + capacity).
    /// This sets capacity, head=0, tail=0, seq=0, and zeroes reserved bytes.
    pub fn initialize_header(buf: &mut [u8], capacity: usize) -> io::Result<()> {
        if buf.len() != HEADER_SIZE + capacity {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "initialize_header: incorrect total buffer length for given capacity",
            ));
        }
        buf[0..4].copy_from_slice(&(capacity as u32).to_le_bytes());
        buf[4..8].copy_from_slice(&0u32.to_le_bytes()); // head
        buf[8..12].copy_from_slice(&0u32.to_le_bytes()); // tail
        buf[12..16].copy_from_slice(&0u32.to_le_bytes()); // seq
        for b in &mut buf[16..32] {
            *b = 0;
        }
        Ok(())
    }

    #[inline]
    fn get_capacity(&self) -> usize {
        self.capacity
    }

    #[inline]
    fn get_head(&self) -> usize {
        u32::from_le_bytes(self.buf[4..8].try_into().unwrap()) as usize % self.capacity
    }

    #[inline]
    fn set_head(&mut self, head: usize) {
        let h = (head % self.capacity) as u32;
        self.buf[4..8].copy_from_slice(&h.to_le_bytes());
    }

    #[inline]
    fn get_tail(&self) -> usize {
        u32::from_le_bytes(self.buf[8..12].try_into().unwrap()) as usize % self.capacity
    }

    #[inline]
    fn set_tail(&mut self, tail: usize) {
        let t = (tail % self.capacity) as u32;
        self.buf[8..12].copy_from_slice(&t.to_le_bytes());
    }

    #[inline]
    fn get_seq(&self) -> u32 {
        u32::from_le_bytes(self.buf[12..16].try_into().unwrap())
    }

    #[inline]
    fn set_seq(&mut self, seq: u32) {
        self.buf[12..16].copy_from_slice(&seq.to_le_bytes());
    }

    /// Returns true if the ring has any records (tail != head).
    #[inline]
    pub fn has_records(&self) -> bool {
        self.get_head() != self.get_tail()
    }

    /// Returns the number of used bytes in the data region.
    #[inline]
    fn used(&self) -> usize {
        let head = self.get_head();
        let tail = self.get_tail();
        (head + self.capacity - tail) % self.capacity
    }

    /// Returns the number of free bytes in the data region.
    #[inline]
    fn free(&self) -> usize {
        self.capacity - self.used()
    }

    /// Copy `len` bytes from ring position `pos` (0..capacity) into `out`,
    /// handling wrap-around. `out` must be at least `len` long.
    fn ring_read(&self, mut pos: usize, out: &mut [u8], len: usize) {
        let mut remaining = len;
        let mut out_offset = 0;
        while remaining > 0 {
            let to_end = self.capacity - (pos % self.capacity);
            let chunk = remaining.min(to_end);
            let abs = data_start() + (pos % self.capacity);
            out[out_offset..out_offset + chunk].copy_from_slice(&self.buf[abs..abs + chunk]);
            remaining -= chunk;
            out_offset += chunk;
            pos = (pos + chunk) % self.capacity;
        }
    }

    /// Copy `src` bytes into ring at position `pos` (0..capacity), handling wrap-around.
    fn ring_write(&mut self, mut pos: usize, src: &[u8]) {
        let mut remaining = src.len();
        let mut src_offset = 0;
        while remaining > 0 {
            let to_end = self.capacity - (pos % self.capacity);
            let chunk = remaining.min(to_end);
            let abs = data_start() + (pos % self.capacity);
            self.buf[abs..abs + chunk].copy_from_slice(&src[src_offset..src_offset + chunk]);
            remaining -= chunk;
            src_offset += chunk;
            pos = (pos + chunk) % self.capacity;
        }
    }

    /// Read a little-endian u32 from the ring at position `pos` (in data space).
    fn ring_read_u32(&self, pos: usize) -> u32 {
        let mut tmp = [0u8; 4];
        self.ring_read(pos, &mut tmp, 4);
        u32::from_le_bytes(tmp)
    }

    /// Write a little-endian u32 to the ring at position `pos` (in data space).
    fn ring_write_u32(&mut self, pos: usize, v: u32) {
        let bytes = v.to_le_bytes();
        self.ring_write(pos, &bytes);
    }

    /// Attempt to skip a single, fully-committed record (advancing the tail).
    /// Returns true if a record was skipped.
    fn skip_record(&mut self) -> bool {
        let tail = self.get_tail();
        if tail == self.get_head() {
            return false; // empty
        }
        // len is at [tail]
        let len = self.ring_read_u32(tail) as usize;
        if len == 0 {
            return false;
        }
        // Trailer is at tail + 4 + len
        let trailer_pos = (tail + 4 + len) % self.capacity;
        let trailer = self.ring_read_u32(trailer_pos) as usize;
        if trailer != len {
            // Not committed yet
            return false;
        }
        // Advance tail past len + header(4) + trailer(4)
        let new_tail = (tail + 4 + len + 4) % self.capacity;
        self.set_tail(new_tail);
        true
    }

    /// Make space for `needed` bytes by skipping fully-committed records
    /// (overwrite-on-full). Returns the number of records skipped.
    fn make_space(&mut self, needed: usize) -> usize {
        let mut skipped = 0;
        while self.free() < needed && self.has_records() {
            if self.skip_record() {
                skipped += 1;
            } else {
                break;
            }
        }
        skipped
    }

    /// Write a payload (already serialized, e.g., FlatBuffers root bytes) as the record payload.
    /// Returns true if written, false if dropped (e.g., couldn't make space).
    pub fn write(&mut self, payload: &[u8]) -> bool {
        let n = payload.len();
        // variable segment after len: type:u16 + pad:u16 + seq:u32 + payload (8 + n)
        let var_len = 8 + n;
        let total = 4 + var_len + 4; // len + var + trailer

        if total > self.capacity {
            // Payload too large for this ring regardless of overwrite policy
            return false;
        }

        // Overwrite-on-full: move tail forward as needed
        self.make_space(total);
        if self.free() < total {
            // Still can't write (next record not committed, etc.)
            return false;
        }

        let head = self.get_head();
        let my_seq = self.get_seq().wrapping_add(1);

        // 1) Write len at [head]
        self.ring_write_u32(head, var_len as u32);

        // 2) Write type=0, pad=0, seq=my_seq just after len
        let var_pos = (head + 4) % self.capacity;
        // type:u16=0
        self.ring_write(var_pos, &0u16.to_le_bytes());
        // pad:u16=0
        self.ring_write(var_pos + 2, &0u16.to_le_bytes());
        // seq:u32
        self.ring_write(var_pos + 4, &my_seq.to_le_bytes());

        // 3) Write payload after the 8 bytes (type+pad+seq)
        self.ring_write((var_pos + 8) % self.capacity, payload);

        // 4) Write trailer len at the end (commit marker)
        let trailer_pos = (head + 4 + var_len) % self.capacity;
        self.ring_write_u32(trailer_pos, var_len as u32);

        // 5) Advance head and bump seq in header
        let new_head = (head + total) % self.capacity;
        self.set_head(new_head);
        self.set_seq(my_seq);

        true
    }

    /// Read the next committed record payload into a Vec<u8>. Returns None if no committed record.
    pub fn read_next(&mut self) -> Option<Vec<u8>> {
        let tail = self.get_tail();
        if tail == self.get_head() {
            return None; // empty
        }

        // Load len and check trailer for commit
        let len = self.ring_read_u32(tail) as usize;
        if len == 0 {
            return None;
        }
        let trailer_pos = (tail + 4 + len) % self.capacity;
        let trailer = self.ring_read_u32(trailer_pos) as usize;
        if trailer != len {
            // Not committed yet
            return None;
        }

        // variable segment starts after len
        let var_pos = (tail + 4) % self.capacity;
        // skip (type:u16 + pad:u16 + seq:u32) = 8 bytes
        let payload_pos = (var_pos + 8) % self.capacity;
        let payload_len = len - 8;

        let mut out = vec![0u8; payload_len];
        self.ring_read(payload_pos, &mut out, payload_len);

        // Advance tail past this record (len + trailer included)
        let new_tail = (tail + 4 + len + 4) % self.capacity;
        self.set_tail(new_tail);

        Some(out)
    }
}

/// A simple bridge that owns two rings: one for reading from the WS worker's
/// "out ring" (messages from the relays), and one for writing to the WS worker's
/// "in ring" (JSON envelopes to send to relays).
pub struct WsRingBridge<'a> {
    pub in_ring: SharedRing<'a>,
    pub out_ring: SharedRing<'a>,
}

impl<'a> WsRingBridge<'a> {
    /// Construct a bridge from two buffers (both header+data).
    pub fn new(in_buf: &'a mut [u8], out_buf: &'a mut [u8]) -> io::Result<Self> {
        Ok(Self {
            in_ring: SharedRing::new(in_buf)?,
            out_ring: SharedRing::new(out_buf)?,
        })
    }

    /// Convenience to initialize ring headers for fresh buffers.
    pub fn initialize_buffers(
        in_buf: &mut [u8],
        in_capacity: usize,
        out_buf: &mut [u8],
        out_capacity: usize,
    ) -> io::Result<()> {
        SharedRing::initialize_header(in_buf, in_capacity)?;
        SharedRing::initialize_header(out_buf, out_capacity)?;
        Ok(())
    }

    /// Read a single message from the out ring. Payload is the raw bytes as written by the TS worker
    /// (e.g., a FlatBuffers-serialized WorkerLine).
    pub fn read_out(&mut self) -> Option<Vec<u8>> {
        self.out_ring.read_next()
    }

    /// Write a JSON envelope to the in ring. The envelope is of the shape:
    ///   { "relays": [ ... ], "frames": [ ... ] }
    /// Each frame is a stringified Nostr client message (e.g., ["REQ", ...], ["EVENT", ...]).
    pub fn write_in_envelope<S1: AsRef<str>, S2: AsRef<str>>(
        &mut self,
        relays: &[S1],
        frames: &[S2],
    ) -> bool {
        let relays_str: Vec<&str> = relays.iter().map(|s| s.as_ref()).collect();
        let frames_str: Vec<&str> = frames.iter().map(|s| s.as_ref()).collect();
        let mut payload = String::from("{\"relays\":[");
        for (i, relay) in relays_str.iter().enumerate() {
            if i > 0 {
                payload.push(',');
            }
            payload.push('"');
            for c in relay.chars() {
                match c {
                    '"' => payload.push_str("\\\""),
                    '\\' => payload.push_str("\\\\"),
                    '\n' => payload.push_str("\\n"),
                    '\r' => payload.push_str("\\r"),
                    '\t' => payload.push_str("\\t"),
                    _ => payload.push(c),
                }
            }
            payload.push('"');
        }
        payload.push_str("],\"frames\":[");
        for (i, frame) in frames_str.iter().enumerate() {
            if i > 0 {
                payload.push(',');
            }
            payload.push('"');
            for c in frame.chars() {
                match c {
                    '"' => payload.push_str("\\\""),
                    '\\' => payload.push_str("\\\\"),
                    '\n' => payload.push_str("\\n"),
                    '\r' => payload.push_str("\\r"),
                    '\t' => payload.push_str("\\t"),
                    _ => payload.push(c),
                }
            }
            payload.push('"');
        }
        payload.push_str("]}");
        self.in_ring.write(payload.as_bytes())
    }
}
