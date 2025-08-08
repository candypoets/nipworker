use js_sys::{SharedArrayBuffer, Uint8Array};
use tracing::{debug, error, warn};

use crate::EOSE;

pub struct SharedBufferManager;

impl SharedBufferManager {
    pub async fn send_eose(shared_buffer: &SharedArrayBuffer, eose: EOSE) {
        let message = crate::WorkerToMainMessage::Eose { data: eose };

        let data = match rmp_serde::to_vec_named(&message) {
            Ok(msgpack) => msgpack,
            Err(e) => {
                error!("Failed to serialize EOSE: {}", e);
                return;
            }
        };

        let _ = Self::write_to_buffer(shared_buffer, &data).await;
    }

    pub async fn send_eoce(shared_buffer: &SharedArrayBuffer) {
        let message = crate::WorkerToMainMessage::Eoce {};

        let data = match rmp_serde::to_vec_named(&message) {
            Ok(msgpack) => msgpack,
            Err(e) => {
                error!("Failed to serialize EOCE: {}", e);
                return;
            }
        };

        let _ = Self::write_to_buffer(shared_buffer, &data).await;
    }

    fn has_buffer_full_marker(
        buffer_uint8: &Uint8Array,
        current_write_pos: usize,
        buffer_length: usize,
    ) -> bool {
        // Check if the last written entry is already a buffer full marker
        if current_write_pos < 5 {
            return false;
        }

        // Read the length of the previous entry (4 bytes before current position)
        let prev_length_pos = current_write_pos - 5; // -5 because marker is 1 byte + 4 byte length
        let prev_length_subarray =
            buffer_uint8.subarray(prev_length_pos as u32, (prev_length_pos + 4) as u32);
        let mut prev_length_bytes = vec![0u8; 4];
        prev_length_subarray.copy_to(&mut prev_length_bytes[..]);

        let mut prev_length_array = [0u8; 4];
        prev_length_array.copy_from_slice(&prev_length_bytes);
        let prev_length = u32::from_le_bytes(prev_length_array);

        // If the previous entry has length 1, check if it's the buffer full marker (0xFF)
        if prev_length == 1 {
            let prev_data_pos = prev_length_pos + 4;
            if prev_data_pos < buffer_length {
                let prev_data_subarray =
                    buffer_uint8.subarray(prev_data_pos as u32, (prev_data_pos + 1) as u32);
                let mut prev_data_bytes = vec![0u8; 1];
                prev_data_subarray.copy_to(&mut prev_data_bytes[..]);

                return prev_data_bytes[0] == 0xFF;
            }
        }

        false
    }

    pub async fn write_to_buffer(shared_buffer: &SharedArrayBuffer, data: &[u8]) {
        // Add safety checks for data size
        if data.len() > 1024 * 1024 {
            // 1MB limit
            warn!("Data too large for SharedArrayBuffer: {} bytes", data.len(),);
            warn!("Dropping message due to size limit");
            return;
        }

        // Get the buffer as Uint8Array for manipulation
        let buffer_uint8 = Uint8Array::new(shared_buffer);
        let buffer_length = buffer_uint8.length() as usize;

        // Read current write position from header (first 4 bytes, little endian)
        let header_subarray = buffer_uint8.subarray(0, 4);
        let mut header_bytes = vec![0u8; 4];
        header_subarray.copy_to(&mut header_bytes[..]);

        let mut header_array = [0u8; 4];
        header_array.copy_from_slice(&header_bytes);
        let current_write_pos = u32::from_le_bytes(header_array) as usize;

        // Safety check for current write position
        if current_write_pos >= buffer_length {
            warn!(
                "Invalid write position {} >= buffer length {}",
                current_write_pos, buffer_length
            );
            warn!("Dropping message due to invalid write position");
            return;
        }

        // Check if we have enough space (4 bytes write position header + 4 bytes length prefix + data)
        let new_write_pos = current_write_pos + 4 + data.len(); // +4 for length prefix
        if new_write_pos > buffer_length {
            // Check if the last written entry is already a buffer full marker
            if Self::has_buffer_full_marker(&buffer_uint8, current_write_pos, buffer_length) {
                warn!("Buffer full, but marker already exists");
                return;
            }
            // Write minimal "buffer full" marker: length=1, data=0xFF
            if current_write_pos + 5 <= buffer_length {
                // 4 bytes length + 1 byte marker
                let length_prefix = 1u32.to_le_bytes(); // Length = 1
                let length_prefix_uint8 = Uint8Array::from(&length_prefix[..]);
                buffer_uint8.set(&length_prefix_uint8, current_write_pos as u32);

                let marker = [0xFF]; // Single byte marker for "buffer full"
                let marker_uint8 = Uint8Array::from(&marker[..]);
                buffer_uint8.set(&marker_uint8, (current_write_pos + 4) as u32);

                // Update write position
                let new_pos = current_write_pos + 5;
                let new_header = (new_pos as u32).to_le_bytes();
                let new_header_uint8 = Uint8Array::from(&new_header[..]);
                buffer_uint8.set(&new_header_uint8, 0);

                warn!("Buffer full, wrote 1-byte marker");
            } else {
                warn!("Buffer completely full, cannot even write marker");
            }
            return;
        }

        // Write the length prefix (4 bytes, little endian) at current write position
        let length_prefix = (data.len() as u32).to_le_bytes();
        let length_prefix_uint8 = Uint8Array::from(&length_prefix[..]);
        buffer_uint8.set(&length_prefix_uint8, current_write_pos as u32);

        // Write the actual data after the length prefix
        let data_uint8 = Uint8Array::from(data);
        buffer_uint8.set(&data_uint8, (current_write_pos + 4) as u32);

        // Update the header with new write position (little endian)
        let new_header = (new_write_pos as u32).to_le_bytes();
        let new_header_uint8 = Uint8Array::from(&new_header[..]);
        buffer_uint8.set(&new_header_uint8, 0);

        debug!(
            "Wrote {} bytes (+ 4 byte length prefix) to SharedArrayBuffer (pos: {} -> {}) and notified waiters",
            data.len(),
            current_write_pos,
            new_write_pos
        );
    }
}
