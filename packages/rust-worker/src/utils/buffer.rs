use std::sync::OnceLock;

use flatbuffers::FlatBufferBuilder;
use js_sys::{SharedArrayBuffer, Uint8Array};
use tracing::{debug, error, info, warn};

use crate::generated::nostr::fb;

static BUFFER_FULL_MARKER: OnceLock<Vec<u8>> = OnceLock::new();

pub struct SharedBufferManager;

impl SharedBufferManager {
    pub async fn send_connection_status(
        shared_buffer: &SharedArrayBuffer,
        relay_url: &str,
        status: &str,
        message: &str,
    ) {
        let mut builder = FlatBufferBuilder::new();

        let relay_url_offset = builder.create_string(&relay_url);
        let status_offset = builder.create_string(&status);
        let message_offset = builder.create_string(&message);

        let conn_status_args = fb::ConnectionStatusArgs {
            relay_url: Some(relay_url_offset),
            status: Some(status_offset),
            message: Some(message_offset),
        };
        let conn_status_offset = fb::ConnectionStatus::create(&mut builder, &conn_status_args);

        let message_args = fb::WorkerMessageArgs {
            type_: fb::MessageType::ConnectionStatus,
            content_type: fb::Message::ConnectionStatus,
            content: Some(conn_status_offset.as_union_value()),
        };
        let root = fb::WorkerMessage::create(&mut builder, &message_args);
        builder.finish(root, None);

        let flatbuffer_data = builder.finished_data();

        let _ = Self::write_to_buffer(shared_buffer, flatbuffer_data).await;
    }

    pub async fn send_eoce(shared_buffer: &SharedArrayBuffer) {
        let mut builder = FlatBufferBuilder::new();

        let subscription_id = builder.create_string(""); // Assuming default or empty subscription_id; adjust if needed
        let eoce_args = fb::EoceArgs {
            subscription_id: Some(subscription_id),
        };
        let eoce_offset = fb::Eoce::create(&mut builder, &eoce_args);

        let message_args = fb::WorkerMessageArgs {
            type_: fb::MessageType::Eoce,
            content_type: fb::Message::Eoce,
            content: Some(eoce_offset.as_union_value()),
        };
        let root = fb::WorkerMessage::create(&mut builder, &message_args);
        builder.finish(root, None);

        let flatbuffer_data = builder.finished_data();

        let _ = Self::write_to_buffer(shared_buffer, flatbuffer_data).await;
    }

    fn has_buffer_full_marker(
        buffer_uint8: &Uint8Array,
        current_write_pos: usize,
        buffer_length: usize,
    ) -> bool {
        // Only check the immediately previous message by jumping back using the known BufferFull size.
        if current_write_pos <= 4 || current_write_pos > buffer_length {
            return false;
        }

        // Initialize and reuse the exact serialized BufferFull marker bytes
        let marker = BUFFER_FULL_MARKER.get_or_init(|| {
            let mut fbb = FlatBufferBuilder::new();
            let buffer_full = {
                let args = fb::BufferFullArgs { dropped_events: 0 };
                fb::BufferFull::create(&mut fbb, &args)
            };
            let worker_msg = {
                let args = fb::WorkerMessageArgs {
                    type_: fb::MessageType::BufferFull,
                    content_type: fb::Message::BufferFull,
                    content: Some(buffer_full.as_union_value()),
                };
                fb::WorkerMessage::create(&mut fbb, &args)
            };
            fbb.finish(worker_msg, None);
            fbb.finished_data().to_vec()
        });
        let marker_len = marker.len();

        // We need at least [len prefix (4)] + [marker payload] behind current_write_pos
        if current_write_pos < 4 + marker_len {
            return false;
        }

        let len_pos = current_write_pos - 4 - marker_len;
        let payload_pos = len_pos + 4;

        if payload_pos + marker_len > buffer_length {
            return false;
        }

        // Verify the length prefix matches the marker size
        let len_sub = buffer_uint8.subarray(len_pos as u32, (len_pos + 4) as u32);
        let mut len_bytes = [0u8; 4];
        len_sub.copy_to(&mut len_bytes[..]);
        let rec_len = u32::from_le_bytes(len_bytes) as usize;

        if rec_len != marker_len {
            return false;
        }

        // Decode the payload and confirm it’s BufferFull
        let last_sub = buffer_uint8.subarray(payload_pos as u32, (payload_pos + marker_len) as u32);
        let mut last_bytes = vec![0u8; marker_len];
        last_sub.copy_to(&mut last_bytes[..]);

        match fb::root_as_worker_message(&last_bytes[..]) {
            Ok(msg) => {
                let is_full = msg.type_() == fb::MessageType::BufferFull
                    || msg.content_type() == fb::Message::BufferFull;
                is_full
            }
            Err(e) => {
                error!(
                    "has_buffer_full_marker: failed to decode last message (start={}, len={}): {}",
                    payload_pos, marker_len, e
                );
                false
            }
        }
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
            if Self::has_buffer_full_marker(&buffer_uint8, current_write_pos, buffer_length) {
                warn!("Buffer full, but marker already exists");
                return;
            }

            // Build bytes once using the same cached marker as the detector
            let data = {
                let marker = BUFFER_FULL_MARKER.get_or_init(|| {
                    let mut fbb = FlatBufferBuilder::new();
                    let buffer_full = {
                        let args = fb::BufferFullArgs { dropped_events: 0 };
                        fb::BufferFull::create(&mut fbb, &args)
                    };
                    let worker_msg = {
                        let args = fb::WorkerMessageArgs {
                            type_: fb::MessageType::BufferFull,
                            content_type: fb::Message::BufferFull,
                            content: Some(buffer_full.as_union_value()),
                        };
                        fb::WorkerMessage::create(&mut fbb, &args)
                    };
                    fbb.finish(worker_msg, None);
                    fbb.finished_data().to_vec()
                });
                marker.as_slice()
            };

            let total_len = data.len() as u32;
            let length_prefix = total_len.to_le_bytes();
            let length_prefix_uint8 = Uint8Array::from(&length_prefix[..]);

            if current_write_pos + 4 + data.len() <= buffer_length {
                // Write length prefix
                buffer_uint8.set(&length_prefix_uint8, current_write_pos as u32);
                // Write the flatbuffer payload
                let payload = Uint8Array::from(data);
                buffer_uint8.set(&payload, (current_write_pos + 4) as u32);

                // Update the header with new write position
                let new_pos = current_write_pos + 4 + data.len();
                let new_header = (new_pos as u32).to_le_bytes();
                let new_header_uint8 = Uint8Array::from(&new_header[..]);
                buffer_uint8.set(&new_header_uint8, 0);

                warn!("Buffer full, wrote WorkerMessage<BufferFull> marker");
            } else {
                warn!("Buffer completely full, cannot write BufferFull message");
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
    }
}
