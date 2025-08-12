// nipworker/packages/rust-worker/src/utils/json.rs
/// Naively extracts the first up to three top-level array elements from a JSON array string.
/// Returns borrowed slices from the original input (zero-copy, fast).
///
/// This assumes the JSON is well-formed per Nostr protocol (no nested arrays before 3rd element,
/// no exotic escapes except `\"` or `\\`).
///
/// The returned slice still has enclosing `"` for string values.
pub fn extract_first_three<'a>(text: &'a str) -> Option<[Option<&'a str>; 3]> {
    let bytes = text.as_bytes();
    if bytes.first()? != &b'[' {
        return None;
    }
    let mut idx = 1; // skip first '['
    let mut results: [Option<&str>; 3] = [None, None, None];
    let mut found = 0;

    while found < 3 && idx < bytes.len() {
        // skip whitespace and commas
        while idx < bytes.len()
            && (bytes[idx] == b' '
                || bytes[idx] == b'\n'
                || bytes[idx] == b'\r'
                || bytes[idx] == b',')
        {
            idx += 1;
        }

        if idx >= bytes.len() || bytes[idx] == b']' {
            break;
        }

        let start = idx;

        if bytes[idx] == b'"' {
            // String element
            idx += 1;
            while idx < bytes.len() {
                match bytes[idx] {
                    b'\\' => idx += 2, // skip escaped char
                    b'"' => {
                        let s = &text[start..=idx];
                        results[found] = Some(s);
                        idx += 1;
                        break;
                    }
                    _ => idx += 1,
                }
            }
        } else if bytes[idx] == b'{' {
            // Object element — find matching closing '}'
            let mut brace_count = 1;
            idx += 1;
            while idx < bytes.len() && brace_count > 0 {
                match bytes[idx] {
                    b'{' => brace_count += 1,
                    b'}' => brace_count -= 1,
                    b'"' => {
                        // skip string inside object
                        idx += 1;
                        while idx < bytes.len() {
                            if bytes[idx] == b'\\' {
                                idx += 2;
                                continue;
                            }
                            if bytes[idx] == b'"' {
                                break;
                            }
                            idx += 1;
                        }
                    }
                    _ => {}
                }
                idx += 1;
            }
            let s = &text[start..idx];
            results[found] = Some(s);
        } else {
            // Primitive (number, bool, null)
            while idx < bytes.len() && bytes[idx] != b',' && bytes[idx] != b']' {
                idx += 1;
            }
            let s = text[start..idx].trim();
            results[found] = Some(s);
        }

        found += 1;
    }

    Some(results)
}

/// Extracts the value of the top-level `"id"` field from a Nostr event JSON string.
/// Uses a lightweight manual scan, no allocations, zero-copy. Browser/WASM friendly.
pub fn extract_event_id<'a>(json: &'a str) -> Option<&'a str> {
    let bytes = json.as_bytes();
    let pat = b"\"id\"";
    let mut i = 0;

    while i + pat.len() <= bytes.len() {
        if &bytes[i..i + pat.len()] == pat {
            // Found `"id"`
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
                    b'\\' => i += 2,                      // skip escape
                    b'"' => return Some(&json[start..i]), // <-- no quotes
                    _ => i += 1,
                }
            }
            return None; // no closing quote
        }
        i += 1;
    }
    None
}
