//! Tagged-header framing for the parser→cache (and mesh→cache) channel.
//!
//! Each message is framed as:
//! `[1-byte tag][4-byte little-endian length][inner FlatBuffer bytes]`
//!
//! - tag [`TAG_PERSIST`]: inner bytes are a standalone `WorkerMessage` root
//! - tag [`TAG_REQUEST`]: inner bytes are a standalone `CacheRequest` root
//!
//! The cache worker roots the inner slice directly and persists the original
//! bytes, so producers keep zero-copy pass-through (no unpack/pack round-trip).

/// Persist message: inner bytes are a standalone `WorkerMessage` root.
pub const TAG_PERSIST: u8 = 0;
/// Request message: inner bytes are a standalone `CacheRequest` root.
pub const TAG_REQUEST: u8 = 1;
/// Byte length of the framing header (1-byte tag + 4-byte length).
pub const HEADER_LEN: usize = 5;

/// Prepend the tagged header to a finished FlatBuffer payload.
pub fn frame(tag: u8, inner: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + inner.len());
    out.push(tag);
    out.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    out.extend_from_slice(inner);
    out
}

/// Split a framed message into its tag and inner payload.
///
/// Returns `None` when the buffer is shorter than the header or the declared
/// length does not match the remaining buffer length.
pub fn split(bytes: &[u8]) -> Option<(u8, &[u8])> {
    if bytes.len() < HEADER_LEN {
        return None;
    }
    let len = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]) as usize;
    if len != bytes.len() - HEADER_LEN {
        return None;
    }
    Some((bytes[0], &bytes[HEADER_LEN..]))
}
