//! Zero-copy scanner for inbound Nostr relay frames.
//!
//! Relay frames are JSON arrays (`["EVENT", <sub_id>, {..}]`, `["OK", ..]`, ...).
//! This module classifies a frame and extracts borrowed byte ranges of its first
//! top-level array elements WITHOUT building a serde_json DOM and without
//! reserializing anything. String escapes are skipped correctly (so an escaped
//! quote inside `content` never terminates a value early), but escape sequences
//! are NOT decoded — returned slices point into the original frame text.

/// A scanned top-level array element: a borrowed slice of the frame text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScannedValue<'a> {
	/// Raw slice of the value; for JSON strings this includes the surrounding quotes.
	pub raw: &'a str,
	/// True when the value is a JSON string.
	pub is_string: bool,
}

impl<'a> ScannedValue<'a> {
	/// Unquoted view for strings (escape sequences are NOT decoded),
	/// or the raw token for numbers/bools/null/objects/arrays.
	#[inline]
	pub fn inner(&self) -> &'a str {
		if self.is_string {
			// raw always starts and ends with an ASCII '"', so this is boundary-safe.
			&self.raw[1..self.raw.len() - 1]
		} else {
			self.raw
		}
	}
}

/// Result of scanning a relay frame: the kind (arr[0], unquoted) plus up to
/// three following elements (arr[1], arr[2], arr[3]).
#[derive(Debug, Clone, Copy)]
pub struct ScannedFrame<'a> {
	/// arr[0]: unquoted for string kinds, raw token otherwise (tolerant).
	pub kind: &'a str,
	/// arr[1], arr[2], arr[3] when present.
	pub args: [Option<ScannedValue<'a>>; 3],
}

#[inline]
fn skip_ws(bytes: &[u8], pos: &mut usize) {
	while *pos < bytes.len() && bytes[*pos].is_ascii_whitespace() {
		*pos += 1;
	}
}

/// Skip a string starting at `bytes[*pos] == '"'`; on return `*pos` is just past
/// the closing quote. Returns false on unterminated string.
#[inline]
fn skip_string(bytes: &[u8], pos: &mut usize) -> bool {
	*pos += 1; // opening quote
	while *pos < bytes.len() {
		match bytes[*pos] {
			b'\\' => *pos = (*pos + 2).min(bytes.len()),
			b'"' => {
				*pos += 1;
				return true;
			}
			_ => *pos += 1,
		}
	}
	false
}

/// Scan one element starting at `*pos` (no leading whitespace) and advance
/// `*pos` past it. Returns None when the element is unterminated.
fn scan_element<'a>(bytes: &[u8], text: &'a str, pos: &mut usize) -> Option<ScannedValue<'a>> {
	let start = *pos;
	match bytes[start] {
		b'"' => {
			if skip_string(bytes, pos) {
				Some(ScannedValue {
					raw: &text[start..*pos],
					is_string: true,
				})
			} else {
				None
			}
		}
		b'{' | b'[' => {
			let mut depth = 0usize;
			while *pos < bytes.len() {
				match bytes[*pos] {
					b'"' => {
						if !skip_string(bytes, pos) {
							return None;
						}
						continue;
					}
					b'{' | b'[' => depth += 1,
					b'}' | b']' => {
						depth -= 1;
						if depth == 0 {
							*pos += 1;
							return Some(ScannedValue {
								raw: &text[start..*pos],
								is_string: false,
							});
						}
					}
					_ => {}
				}
				*pos += 1;
			}
			None // unbalanced
		}
		_ => {
			// Primitive token (number, bool, null). Multibyte UTF-8 continuation
			// bytes never match these ASCII delimiters, so slicing stays safe.
			while *pos < bytes.len()
				&& !matches!(bytes[*pos], b',' | b']' | b' ' | b'\t' | b'\n' | b'\r')
			{
				*pos += 1;
			}
			if *pos == start {
				None
			} else {
				Some(ScannedValue {
					raw: &text[start..*pos],
					is_string: false,
				})
			}
		}
	}
}

/// Scan a relay frame, returning the kind and byte ranges of arr[1..=3].
///
/// Tolerant by design (like the previous `extract_first_three` fallback):
/// it scans element prefixes and ignores trailing garbage instead of
/// rejecting the whole frame. Returns None only when the input is not an
/// array or arr[0] is unterminated.
pub fn scan_relay_frame(text: &str) -> Option<ScannedFrame<'_>> {
	let bytes = text.as_bytes();
	let mut pos = 0;
	skip_ws(bytes, &mut pos);
	if pos >= bytes.len() || bytes[pos] != b'[' {
		return None;
	}
	pos += 1;
	skip_ws(bytes, &mut pos);
	if pos >= bytes.len() || bytes[pos] == b']' {
		return None;
	}

	let first = scan_element(bytes, text, &mut pos)?;
	let kind = first.inner();

	let mut args: [Option<ScannedValue<'_>>; 3] = [None, None, None];
	let mut filled = 0usize;
	while filled < 3 {
		skip_ws(bytes, &mut pos);
		if pos >= bytes.len() || bytes[pos] == b']' {
			break;
		}
		if bytes[pos] != b',' {
			break; // malformed separator; keep what we have
		}
		pos += 1;
		skip_ws(bytes, &mut pos);
		if pos >= bytes.len() || bytes[pos] == b']' {
			break; // trailing comma / end of array
		}
		let Some(v) = scan_element(bytes, text, &mut pos) else {
			break;
		};
		args[filled] = Some(v);
		filled += 1;
	}

	Some(ScannedFrame { kind, args })
}

#[cfg(test)]
mod tests {
	use super::*;

	fn arg<'a>(frame: &ScannedFrame<'a>, i: usize) -> ScannedValue<'a> {
		frame.args[i].expect("expected element")
	}

	#[test]
	fn test_event_frame_extracts_kind_sub_and_event_range() {
		let text = r#"["EVENT","sub1",{"id":"abc","pubkey":"pk","kind":1,"content":"hi","tags":[],"created_at":1,"sig":"s"}]"#;
		let scan = scan_relay_frame(text).unwrap();
		assert_eq!(scan.kind, "EVENT");
		assert_eq!(arg(&scan, 0).inner(), "sub1");
		let event = arg(&scan, 1);
		assert!(!event.is_string);
		assert_eq!(
			event.raw,
			r#"{"id":"abc","pubkey":"pk","kind":1,"content":"hi","tags":[],"created_at":1,"sig":"s"}"#
		);
	}

	#[test]
	fn test_event_with_escaped_quotes_in_content() {
		let text = r#"["EVENT","s",{"id":"a","content":"he said \"hi\" and \\ done"}]"#;
		let scan = scan_relay_frame(text).unwrap();
		assert_eq!(scan.kind, "EVENT");
		assert_eq!(
			arg(&scan, 1).raw,
			r#"{"id":"a","content":"he said \"hi\" and \\ done"}"#
		);
	}

	#[test]
	fn test_event_with_nested_arrays_and_braces_in_strings() {
		let text =
			r#"["EVENT","s",{"tags":[["p","x"],["e","y","}"],"quoted"],"content":"} ] [ {","kind":1}]"#;
		let scan = scan_relay_frame(text).unwrap();
		assert_eq!(
			arg(&scan, 1).raw,
			r#"{"tags":[["p","x"],["e","y","}"],"quoted"],"content":"} ] [ {","kind":1}"#
		);
	}

	#[test]
	fn test_non_event_frames() {
		let eose = scan_relay_frame(r#"["EOSE","sub9"]"#).unwrap();
		assert_eq!(eose.kind, "EOSE");
		assert_eq!(arg(&eose, 0).inner(), "sub9");
		assert!(eose.args[1].is_none());

		let ok = scan_relay_frame(r#"["OK","ev1",true,"duplicate: already have this"]"#).unwrap();
		assert_eq!(ok.kind, "OK");
		assert_eq!(arg(&ok, 0).inner(), "ev1");
		assert_eq!(arg(&ok, 1).raw, "true");
		assert!(!arg(&ok, 1).is_string);
		assert_eq!(arg(&ok, 2).inner(), "duplicate: already have this");

		let ok_synth = scan_relay_frame(r#"["OK","sub","SUBSCRIBED"]"#).unwrap();
		assert_eq!(arg(&ok_synth, 1).inner(), "SUBSCRIBED");
		assert!(arg(&ok_synth, 1).is_string);

		let notice = scan_relay_frame(r#"["NOTICE","rate limited"]"#).unwrap();
		assert_eq!(notice.kind, "NOTICE");
		assert_eq!(arg(&notice, 0).inner(), "rate limited");

		let auth = scan_relay_frame(r#"["AUTH","challenge-abc"]"#).unwrap();
		assert_eq!(auth.kind, "AUTH");
		assert_eq!(arg(&auth, 0).inner(), "challenge-abc");

		let closed = scan_relay_frame(r#"["CLOSED","sub1","error: bad filter"]"#).unwrap();
		assert_eq!(closed.kind, "CLOSED");
		assert_eq!(arg(&closed, 0).inner(), "sub1");
		assert_eq!(arg(&closed, 1).inner(), "error: bad filter");

		let count = scan_relay_frame(r#"["COUNT","sub1",{"count":42}]"#).unwrap();
		assert_eq!(count.kind, "COUNT");
		assert_eq!(arg(&count, 0).inner(), "sub1");
		assert_eq!(arg(&count, 1).raw, r#"{"count":42}"#);
	}

	#[test]
	fn test_whitespace_is_tolerated() {
		let scan = scan_relay_frame("[ \"EVENT\" , \"s\" , { \"a\" : 1 } ]").unwrap();
		assert_eq!(scan.kind, "EVENT");
		assert_eq!(arg(&scan, 0).inner(), "s");
		assert_eq!(arg(&scan, 1).raw, "{ \"a\" : 1 }");
	}

	#[test]
	fn test_malformed_inputs() {
		assert!(scan_relay_frame("").is_none());
		assert!(scan_relay_frame("not json").is_none());
		assert!(scan_relay_frame("[]").is_none());
		assert!(scan_relay_frame(r#"{"EVENT":"x"}"#).is_none());
		// Unterminated first string -> no frame
		assert!(scan_relay_frame(r#"["EVENT"#).is_none());
		// Tolerant: keeps the kind even when the rest is garbage
		let partial = scan_relay_frame(r#"["NOTICE","oops" trailing"#).unwrap();
		assert_eq!(partial.kind, "NOTICE");
		assert_eq!(arg(&partial, 0).inner(), "oops");
	}
}
