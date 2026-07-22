//! Cross-relay EVENT dedup at the connections layer.
//!
//! A REQ is fanned out to N relays under the same subscription id, so the same
//! event arrives once per relay per subId. Suppressing duplicate EVENT frames
//! here keeps them off the connections→parser channel entirely; the parser
//! pipeline keeps its own `seen_ids` set as a safety net for cache-sourced
//! events and any path that bypasses the connections worker.
//!
//! The dedup key is (subId, event id): the same event legitimately belongs to
//! multiple subscriptions and must be delivered once per subscription, never
//! once globally. State is bounded (ring-evicted at `MAX_DEDUP_IDS_PER_SUB`
//! per subscription, mirroring the `MeshWatch` pattern in cache_worker.rs)
//! and freed by the connections worker when the subscription is CLOSEd.

use crate::transport::frame_scan::{scan_relay_frame, ScannedFrame};
use rustc_hash::FxHashSet;
use std::collections::VecDeque;

/// Maximum remembered event ids per subscription; oldest are evicted FIFO.
/// Worst-case memory: subscriptions × 4096 × 32 bytes.
pub const MAX_DEDUP_IDS_PER_SUB: usize = 4096;

/// Bounded per-subscription set of already-forwarded event ids.
pub struct SubDedup {
	ids: FxHashSet<[u8; 32]>,
	order: VecDeque<[u8; 32]>,
}

impl SubDedup {
	pub fn new() -> Self {
		Self {
			ids: FxHashSet::default(),
			order: VecDeque::new(),
		}
	}

	/// Returns true the first time an id is seen (and records it), false for
	/// duplicates. When the ring exceeds `MAX_DEDUP_IDS_PER_SUB` the oldest id
	/// is evicted; an evicted id may be forwarded again, where the parser-side
	/// dedup acts as the safety net.
	pub fn mark(&mut self, id: [u8; 32]) -> bool {
		if !self.ids.insert(id) {
			return false;
		}
		self.order.push_back(id);
		if self.order.len() > MAX_DEDUP_IDS_PER_SUB {
			if let Some(oldest) = self.order.pop_front() {
				self.ids.remove(&oldest);
			}
		}
		true
	}
}

/// Extracts the value of the top-level `"id"` field from a Nostr event JSON
/// string. Lightweight manual scan, no allocations, zero-copy. Mirrors
/// `parser_utils::json::extract_event_id`, which is gated behind the `parser`
/// feature and therefore unavailable to the connections worker.
fn extract_event_id<'a>(json: &'a str) -> Option<&'a str> {
	let bytes = json.as_bytes();
	let pat = b"\"id\"";
	let mut i = 0;

	while i + pat.len() <= bytes.len() {
		if &bytes[i..i + pat.len()] == pat {
			i += pat.len();
			// skip spaces and colon
			while i < bytes.len() && (bytes[i] == b' ' || bytes[i] == b'\t' || bytes[i] == b':') {
				i += 1;
			}
			// must be starting a string
			if i >= bytes.len() || bytes[i] != b'"' {
				return None;
			}
			i += 1;
			let start = i;
			while i < bytes.len() {
				match bytes[i] {
					b'\\' => i += 2,
					b'"' => return Some(&json[start..i]),
					_ => i += 1,
				}
			}
			return None;
		}
		i += 1;
	}
	None
}

/// Extract the 32-byte event id from an already-scanned `["EVENT", <sub_id>,
/// {..}]` relay frame. Returns None for non-EVENT frames or frames whose id
/// cannot be extracted/decoded — callers must forward those untouched so the
/// parser handles them exactly as before.
pub fn scanned_event_id(scan: &ScannedFrame<'_>) -> Option<[u8; 32]> {
	if scan.kind != "EVENT" {
		return None;
	}
	let event_obj = scan.args[1].map(|v| v.raw)?;
	let id_hex = extract_event_id(event_obj)?;
	let mut id = [0u8; 32];
	hex::decode_to_slice(id_hex, &mut id).ok()?;
	Some(id)
}

/// Extract the 32-byte event id from a raw `["EVENT", <sub_id>, {..}]` relay
/// frame. Returns None for non-EVENT frames or frames whose id cannot be
/// extracted/decoded — callers must forward those untouched so the parser
/// handles them exactly as before.
pub fn event_frame_id(frame: &str) -> Option<[u8; 32]> {
	let scan = scan_relay_frame(frame)?;
	scanned_event_id(&scan)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn frame_with_id(id_hex: &str) -> String {
		format!(
			r#"["EVENT","sub1",{{"id":"{}","pubkey":"pk","kind":1,"content":"hi","tags":[],"created_at":1,"sig":"s"}}]"#,
			id_hex
		)
	}

	#[test]
	fn extracts_id_from_event_frame() {
		let id_hex = "ab".repeat(32);
		let frame = frame_with_id(&id_hex);
		let id = event_frame_id(&frame).expect("should extract id");
		assert_eq!(id, [0xab; 32]);
	}

	#[test]
	fn ignores_non_event_frames() {
		assert!(event_frame_id(r#"["EOSE","sub1"]"#).is_none());
		assert!(event_frame_id(r#"["OK","sub1","SUBSCRIBED"]"#).is_none());
		assert!(event_frame_id(r#"["CLOSED","sub1","error"]"#).is_none());
		assert!(event_frame_id(r#"["NOTICE","hi"]"#).is_none());
	}

	#[test]
	fn malformed_or_missing_id_returns_none() {
		// Not valid hex -> None (forward untouched, parser decides)
		assert!(event_frame_id(&frame_with_id("not-hex")).is_none());
		// No id field at all
		assert!(event_frame_id(r#"["EVENT","s",{"kind":1}]"#).is_none());
		// Garbage
		assert!(event_frame_id("not json").is_none());
	}

	#[test]
	fn first_seen_true_duplicate_false() {
		let mut dedup = SubDedup::new();
		assert!(dedup.mark([1; 32]));
		assert!(!dedup.mark([1; 32]));
		assert!(dedup.mark([2; 32]));
	}

	#[test]
	fn ring_evicts_oldest_beyond_cap() {
		let mut dedup = SubDedup::new();
		for i in 0..MAX_DEDUP_IDS_PER_SUB {
			let mut id = [0u8; 32];
			id[0..8].copy_from_slice(&(i as u64).to_le_bytes());
			assert!(dedup.mark(id));
		}
		assert_eq!(dedup.ids.len(), MAX_DEDUP_IDS_PER_SUB);

		// One more insert evicts the oldest (i = 0)
		let mut extra = [0u8; 32];
		extra[0..8].copy_from_slice(&(MAX_DEDUP_IDS_PER_SUB as u64).to_le_bytes());
		assert!(dedup.mark(extra));
		assert_eq!(dedup.ids.len(), MAX_DEDUP_IDS_PER_SUB);

		// The evicted id is seen as new again (parser dedup is the safety net).
		// Note: re-inserting it evicts the next-oldest (i = 1) in turn.
		let mut oldest = [0u8; 32];
		oldest[0..8].copy_from_slice(&0u64.to_le_bytes());
		assert!(dedup.mark(oldest));
		// ...while a still-resident id is still a duplicate
		let mut resident = [0u8; 32];
		resident[0..8].copy_from_slice(&2u64.to_le_bytes());
		assert!(!dedup.mark(resident));
	}
}
