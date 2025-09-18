use crate::parser::{ParserError, Result};

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

pub struct BaseJsonParser<'a> {
    pub bytes: &'a [u8],
    pub pos: usize,
}

impl<'a> BaseJsonParser<'a> {
    #[inline(always)]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    #[inline(always)]
    pub fn peek(&self) -> u8 {
        self.bytes[self.pos]
    }

    #[inline(always)]
    pub fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    #[inline(always)]
    pub fn expect_byte(&mut self, expected: u8) -> Result<()> {
        if self.pos >= self.bytes.len() || self.bytes[self.pos] != expected {
            return Err(ParserError::InvalidFormat(format!(
                "Unexpected byte at position {}",
                self.pos
            )));
        }
        self.pos += 1;
        Ok(())
    }

    #[inline(always)]
    pub fn parse_string(&mut self) -> Result<&'a str> {
        self.expect_byte(b'"')?;
        let start = self.pos;

        while self.pos < self.bytes.len() {
            match self.bytes[self.pos] {
                b'"' => {
                    let result =
                        unsafe { std::str::from_utf8_unchecked(&self.bytes[start..self.pos]) };
                    self.pos += 1;
                    return Ok(result);
                }
                b'\\' => {
                    // Skip escaped character
                    if self.pos + 1 < self.bytes.len() {
                        self.pos += 2;
                    } else {
                        self.pos += 1;
                    }
                }
                _ => self.pos += 1,
            }
        }

        Err(ParserError::InvalidFormat(
            "Unterminated string".to_string(),
        ))
    }

    #[inline(always)]
    pub fn parse_u64(&mut self) -> Result<u64> {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }

        if start == self.pos {
            return Err(ParserError::InvalidFormat("Expected number".to_string()));
        }

        let num_str = unsafe { std::str::from_utf8_unchecked(&self.bytes[start..self.pos]) };
        num_str
            .parse()
            .map_err(|_| ParserError::InvalidFormat("Invalid number".to_string()))
    }

    #[inline(always)]
    pub fn parse_i32(&mut self) -> Result<i32> {
        self.parse_u64().map(|n| n as i32)
    }

    #[inline(always)]
    pub fn skip_value(&mut self) -> Result<()> {
        match self.peek() {
            b'"' => {
                self.parse_string()?;
            }
            b'[' => self.skip_array()?,
            b'{' => self.skip_object()?,
            b't' | b'f' => self.skip_bool()?,
            b'n' => self.skip_null()?,
            _ => self.skip_number()?,
        }
        Ok(())
    }

    #[inline(always)]
    pub fn skip_array(&mut self) -> Result<()> {
        self.expect_byte(b'[')?;
        let mut depth = 1;

        while self.pos < self.bytes.len() && depth > 0 {
            match self.bytes[self.pos] {
                b'[' => depth += 1,
                b']' => depth -= 1,
                _ => {}
            }
            self.pos += 1;
        }
        Ok(())
    }

    #[inline(always)]
    pub fn skip_object(&mut self) -> Result<()> {
        self.expect_byte(b'{')?;
        let mut depth = 1;

        while self.pos < self.bytes.len() && depth > 0 {
            match self.bytes[self.pos] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            self.pos += 1;
        }
        Ok(())
    }

    #[inline(always)]
    pub fn skip_bool(&mut self) -> Result<()> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
        } else {
            return Err(ParserError::InvalidFormat("Invalid boolean".to_string()));
        }
        Ok(())
    }

    #[inline(always)]
    pub fn skip_null(&mut self) -> Result<()> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
        } else {
            return Err(ParserError::InvalidFormat("Invalid null".to_string()));
        }
        Ok(())
    }

    #[inline(always)]
    pub fn skip_number(&mut self) -> Result<()> {
        while self.pos < self.bytes.len()
            && (self.bytes[self.pos].is_ascii_digit()
                || self.bytes[self.pos] == b'.'
                || self.bytes[self.pos] == b'-'
                || self.bytes[self.pos] == b'+')
        {
            self.pos += 1;
        }
        Ok(())
    }

    #[inline(always)]
    pub fn skip_comma_or_end(&mut self) -> Result<()> {
        self.skip_whitespace();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b',' {
            self.pos += 1;
        }
        Ok(())
    }

    #[inline(always)]
    pub fn parse_raw_json_value(&mut self) -> Result<&'a str> {
        let start = self.pos;
        let mut depth = 0;
        let mut in_string = false;
        let mut escaped = false;

        while self.pos < self.bytes.len() {
            let ch = self.bytes[self.pos];

            match ch {
                b'"' => {
                    if !escaped {
                        in_string = !in_string;
                    }
                }
                b'\\' => {
                    escaped = !escaped;
                    self.pos += 1;
                    continue;
                }
                b'{' | b'[' => {
                    if !in_string {
                        depth += 1;
                    }
                }
                b'}' | b']' => {
                    if !in_string {
                        depth -= 1;
                        if depth == 0 {
                            self.pos += 1;
                            return Ok(unsafe {
                                std::str::from_utf8_unchecked(&self.bytes[start..self.pos])
                            });
                        }
                    }
                }
                b',' => {
                    if !in_string && depth == 0 {
                        return Ok(unsafe {
                            std::str::from_utf8_unchecked(&self.bytes[start..self.pos])
                        });
                    }
                }
                _ => {}
            }

            escaped = false;
            self.pos += 1;
        }

        Err(ParserError::InvalidFormat("Invalid JSON value".to_string()))
    }
}
