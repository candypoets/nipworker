use js_sys::{DataView, SharedArrayBuffer, Uint8Array};
use std::cell::RefCell;
use std::cmp;
use std::rc::Rc;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct ByteRingBuffer {
    sab: SharedArrayBuffer,
    data_view: DataView,
    data_start: u32,
    capacity: u32,
    dropped: Rc<RefCell<u32>>,
    head: Rc<RefCell<u32>>,
    tail: Rc<RefCell<u32>>,
    seq: Rc<RefCell<u32>>,
}

#[wasm_bindgen]
impl ByteRingBuffer {
    #[wasm_bindgen(constructor)]
    pub fn new(buffer: SharedArrayBuffer) -> Self {
        // Fixed: byte_length as Option<u32>
        let data_view = DataView::new(&buffer, 0u32, Some(buffer.byte_length() as u32));
        let capacity = data_view.get_uint32(0u32, true); // Fixed: explicit u32, true for little-endian
        Self {
            sab: buffer,
            data_view,
            data_start: 32,
            capacity,
            dropped: Rc::new(RefCell::new(0u32)),
            head: Rc::new(RefCell::new(data_view.get_uint32(4u32, true) % capacity)),
            tail: Rc::new(RefCell::new(data_view.get_uint32(8u32, true) % capacity)),
            seq: Rc::new(RefCell::new(data_view.get_uint32(12u32, true))),
        }
    }

    pub fn get_free_space(&self) -> u32 {
        let head = self.get_head();
        let tail = self.get_tail();
        let used = (head + self.capacity - tail) % self.capacity;
        self.capacity.saturating_sub(used)
    }

    pub fn get_dropped(&self) -> u32 {
        *self.dropped.borrow()
    }

    pub fn has_records(&self) -> bool {
        self.get_head() != self.get_tail()
    }

    // Fixed: &mut self for write (modifies head/seq/dropped)
    pub fn write(&mut self, payload: &Uint8Array) -> i32 {
        let n = payload.length() as u32;
        let len = 8u32.saturating_add(n);
        let total_size = 4u32.saturating_add(len).saturating_add(4u32);

        let mut dropped_this_write = 0u32;
        while self.get_free_space() < total_size {
            if !self.skip_record() {
                *self.dropped.borrow_mut() = self
                    .dropped
                    .borrow()
                    .saturating_add(dropped_this_write)
                    .saturating_add(1);
                return -1i32;
            }
            dropped_this_write = dropped_this_write.saturating_add(1);
        }

        let my_seq = self.get_seq().saturating_add(1);
        self.set_seq(my_seq);

        let mut write_pos = self.get_head();

        // Fixed: set_uint32 takes 3 args (offset, value, little_endian)
        self.data_view
            .set_uint32(self.data_start.saturating_add(write_pos), len, true);
        write_pos = (write_pos.saturating_add(4)) % self.capacity;

        self.data_view
            .set_uint16(self.data_start.saturating_add(write_pos), 0u16, true);
        write_pos = (write_pos.saturating_add(2)) % self.capacity;

        self.data_view
            .set_uint16(self.data_start.saturating_add(write_pos), 0u16, true);
        write_pos = (write_pos.saturating_add(2)) % self.capacity;

        self.data_view
            .set_uint32(self.data_start.saturating_add(write_pos), my_seq, true);
        write_pos = (write_pos.saturating_add(4)) % self.capacity;

        self.copy_bytes(write_pos, payload, 0u32, n);
        write_pos = (write_pos.saturating_add(n)) % self.capacity;

        self.data_view
            .set_uint32(self.data_start.saturating_add(write_pos), len, true);
        write_pos = (write_pos.saturating_add(4)) % self.capacity;

        self.set_head(write_pos);

        *self.dropped.borrow_mut() = self.dropped.borrow().saturating_add(dropped_this_write);
        my_seq as i32
    }

    // Fixed: &mut self for read (modifies tail)
    pub fn read(&mut self) -> Option<Uint8Array> {
        let mut read_pos = self.get_tail();
        if read_pos == self.get_head() {
            return None;
        }

        let len = self
            .data_view
            .get_uint32(self.data_start.saturating_add(read_pos), true);
        if len == 0 {
            return None;
        }

        let trailer_pos = (read_pos.saturating_add(4).saturating_add(len)) % self.capacity;
        let trailer = self
            .data_view
            .get_uint32(self.data_start.saturating_add(trailer_pos), true);

        if trailer != len {
            return None;
        }

        let variable_len = len as usize;
        let mut variable = Uint8Array::new_with_length(variable_len as u32); // Mutable for filling
        self.copy_from_ring(
            (read_pos.saturating_add(4)) % self.capacity,
            &mut variable,
            0u32,
            len,
        );

        let payload = variable.subarray(8u32, len);

        let advance = 4u32.saturating_add(len).saturating_add(4u32);
        self.set_tail((self.get_tail().saturating_add(advance)) % self.capacity);

        Some(payload)
    }

    // Fixed: &mut self for skip_record (modifies tail)
    fn skip_record(&mut self) -> bool {
        let mut read_pos = self.get_tail();
        if read_pos == self.get_head() {
            return false;
        }

        let len = self
            .data_view
            .get_uint32(self.data_start.saturating_add(read_pos), true);
        if len == 0 {
            return false;
        }

        let trailer_pos = (read_pos.saturating_add(4).saturating_add(len)) % self.capacity;
        let trailer = self
            .data_view
            .get_uint32(self.data_start.saturating_add(trailer_pos), true);

        if trailer != len {
            return false;
        }

        let advance = 4u32.saturating_add(len).saturating_add(4u32);
        self.set_tail((self.get_tail().saturating_add(advance)) % self.capacity);
        true
    }

    fn get_head(&self) -> u32 {
        *self.head.borrow()
    }

    // Fixed: &mut self for set_head (modifies DataView and RefCell)
    fn set_head(&mut self, value: u32) {
        let new_head = value % self.capacity;
        *self.head.borrow_mut() = new_head;
        self.data_view.set_uint32(4u32, new_head, true); // Fixed: 3 args for set_uint32
    }

    fn get_tail(&self) -> u32 {
        *self.tail.borrow()
    }

    // Fixed: &mut self for set_tail
    fn set_tail(&mut self, value: u32) {
        let new_tail = value % self.capacity;
        *self.tail.borrow_mut() = new_tail;
        self.data_view.set_uint32(8u32, new_tail, true);
    }

    fn get_seq(&self) -> u32 {
        *self.seq.borrow()
    }

    // Fixed: &mut self for set_seq
    fn set_seq(&mut self, value: u32) {
        *self.seq.borrow_mut() = value;
        self.data_view.set_uint32(12u32, value, true);
    }

    // Fixed: &mut self for copy_bytes (modifies sab)
    fn copy_bytes(
        &mut self,
        target_pos: u32,
        source: &Uint8Array,
        source_offset: u32,
        length: u32,
    ) {
        let mut remaining = length;
        let mut src_offset = source_offset;
        let mut tgt = target_pos;

        while remaining > 0 {
            let space_to_end = self.capacity - (tgt % self.capacity);
            let chunk_size = cmp::min(remaining, space_to_end);
            let tgt_abs = self.data_start + (tgt % self.capacity);
            let src_chunk = source.subarray(src_offset, src_offset + chunk_size);
            // Fixed: Use view_mut on sab for writing
            let tgt_view = unsafe {
                Uint8Array::view_mut(self.sab.as_ref(), tgt_abs as usize, chunk_size as usize)
            };
            tgt_view.copy_from(&src_chunk);
            remaining = remaining.saturating_sub(chunk_size);
            src_offset = src_offset.saturating_add(chunk_size);
            tgt = tgt.saturating_add(chunk_size);
        }
    }

    // Fixed: &self (read-only), but target &mut Uint8Array for filling
    fn copy_from_ring(
        &self,
        source_pos: u32,
        target: &mut Uint8Array,
        target_offset: u32,
        length: u32,
    ) {
        let mut remaining = length;
        let mut tgt_offset = target_offset;
        let mut src = source_pos;

        while remaining > 0 {
            let space_to_end = self.capacity - (src % self.capacity);
            let chunk_size = cmp::min(remaining, space_to_end);
            let src_abs = self.data_start + (src % self.capacity);
            // Fixed: view (immutable) on sab
            let src_chunk = unsafe {
                Uint8Array::view(self.sab.as_ref(), src_abs as usize, chunk_size as usize)
            };
            // Fixed: set on mutable target
            target.set(&src_chunk, tgt_offset);
            remaining = remaining.saturating_sub(chunk_size);
            tgt_offset = tgt_offset.saturating_add(chunk_size);
            src = src.saturating_add(chunk_size);
        }
    }
}
