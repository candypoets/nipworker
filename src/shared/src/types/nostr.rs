use crate::{generated::nostr::fb, types::TypesError, utils::BaseJsonParser};
use k256::schnorr::{Signature, SigningKey, VerifyingKey};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt::Write};

type Result<T> = std::result::Result<T, TypesError>;

// Import the signature traits we need
use k256::schnorr::signature::{Signer, Verifier};

// Just re-export what we need from k256
pub const SECP256K1: k256::Secp256k1 = k256::Secp256k1;

// ============================================================================
// Basic Types - Just byte arrays
// ============================================================================
#[derive(Clone, Copy)]
pub struct EventId(pub [u8; 32]);

impl EventId {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes =
            hex::decode(s).map_err(|_| TypesError::InvalidFormat("Invalid hex".to_string()))?;
        if bytes.len() != 32 {
            return Err(TypesError::InvalidFormat("Invalid event ID".to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(EventId(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }

    pub fn to_string(&self) -> String {
        self.to_hex()
    }
}

#[derive(Clone)]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes =
            hex::decode(s).map_err(|_| TypesError::InvalidFormat("Invalid hex".to_string()))?;
        if bytes.len() != 32 {
            return Err(TypesError::InvalidFormat("Invalid pubkey".to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(PublicKey(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn to_string(&self) -> String {
        self.to_hex()
    }
}
#[derive(Clone)]
pub struct SecretKey(pub [u8; 32]);

impl SecretKey {
    pub fn from_hex(s: &str) -> Result<Self> {
        let bytes =
            hex::decode(s).map_err(|_| TypesError::InvalidFormat("Invalid hex".to_string()))?;
        if bytes.len() != 32 {
            return Err(TypesError::InvalidFormat("Invalid secret key".to_string()));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(SecretKey(arr))
    }

    pub fn to_hex(&self) -> String {
        hex::encode(self.0)
    }

    pub fn public_key(&self, _secp: &k256::Secp256k1) -> PublicKey {
        let signing_key = SigningKey::from_bytes(&self.0).unwrap();
        let verifying_key = signing_key.verifying_key();
        PublicKey(verifying_key.to_bytes().into())
    }

    pub fn display_secret(&self) -> String {
        self.to_hex()
    }
}

#[derive(Clone)]
pub struct Keys {
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
}

impl Keys {
    pub fn new(secret_key: SecretKey) -> Self {
        let public_key = secret_key.public_key(&SECP256K1);
        Self {
            secret_key,
            public_key,
        }
    }

    pub fn parse(nsec: &str) -> Result<Self> {
        // Check if it starts with "nsec1" for bech32 format
        if nsec.starts_with("nsec1") {
            // For now, return error since bech32 is not implemented
            return Err(TypesError::InvalidFormat(
                "Bech32 nsec parsing not implemented".to_string(),
            ));
        }

        // Otherwise treat it as hex
        let secret_key = SecretKey::from_hex(nsec)?;
        Ok(Self::new(secret_key))
    }

    pub fn generate() -> Self {
        let signing_key = SigningKey::random(&mut k256::elliptic_curve::rand_core::OsRng);
        let secret_bytes: [u8; 32] = signing_key.to_bytes().into();
        Self::new(SecretKey(secret_bytes))
    }

    pub fn secret_key(&self) -> Result<&SecretKey> {
        Ok(&self.secret_key)
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.public_key.0)
    }
}

// ============================================================================
// Event & Filter
// ============================================================================

// Just use u64 directly for timestamp
pub type Timestamp = u64;

pub struct Template {
    pub kind: Kind,
    pub content: String,
    pub tags: Vec<Vec<String>>,
    pub created_at: Timestamp, // Added created_at field of type Timestamp (u64)
}

impl Template {
    pub fn new(kind: Kind, content: String, tags: Vec<Vec<String>>) -> Self {
        Template {
            kind,
            content,
            tags,
            created_at: if 0 == 0 { timestamp_now() } else { 0 },
        }
    }

    pub fn from_flatbuffer(fb_template: &fb::Template) -> Self {
        let mut tags = Vec::new();
        let fb_tags = fb_template.tags();
        for i in 0..fb_tags.len() {
            let tag_vec = fb_tags.get(i);
            if let Some(items) = tag_vec.items() {
                let tag: Vec<String> = items.iter().map(|s| s.to_string()).collect();
                tags.push(tag);
            }
        }

        Template {
            kind: fb_template.kind(),
            content: fb_template.content().to_string(),
            tags,
            created_at: fb_template.created_at() as u64,
        }
    }

    /// Serialize Template to a compact JSON string: {"kind":<u32>,"content":"<escaped>","tags":[...]}
    pub fn to_json(&self) -> String {
        let tags_json = NostrTags(self.tags.clone()).to_json();
        let mut result = String::with_capacity(32 + self.content.len() * 2 + tags_json.len() + 20);
        use core::fmt::Write;
        write!(
            result,
            r#"{{"kind":{},"content":"{}","tags":{},"created_at":{}}}"#,
            self.kind,
            Self::escape_string(&self.content),
            tags_json,
            self.created_at
        )
        .unwrap();
        result
    }

    /// Parse Template from a minimal JSON object containing "kind", "content", and "tags".
    /// Uses a lightweight scan and NostrTags::from_json for the tags array.
    pub fn from_json(json: &str) -> Result<Self> {
        // kind
        let kind = {
            if let Some(i) = json.find("\"kind\"") {
                let tail = &json[i + 6..];
                // Skip spaces and colon
                let bytes = tail.as_bytes();
                let mut j = 0usize;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b':')
                {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if start == j {
                    return Err(TypesError::InvalidFormat("Invalid kind".to_string()));
                }
                let num_str = &tail[start..j];
                num_str
                    .parse::<u32>()
                    .map_err(|_| TypesError::InvalidFormat("Invalid kind".to_string()))?
            } else {
                return Err(TypesError::InvalidFormat("Missing kind".to_string()));
            }
        };

        // content
        let content = {
            if let Some(i) = json.find("\"content\"") {
                let tail = &json[i + 9..];
                let qpos = tail.find('"').ok_or_else(|| {
                    TypesError::InvalidFormat("Missing content string".to_string())
                })?;
                let bytes = tail.as_bytes();
                let mut idx = qpos + 1;
                let mut out = String::new();
                let mut escaped = false;
                while idx < tail.len() {
                    let ch = bytes[idx];
                    if escaped {
                        match ch {
                            b'"' => out.push('"'),
                            b'\\' => out.push('\\'),
                            b'n' => out.push('\n'),
                            b'r' => out.push('\r'),
                            b't' => out.push('\t'),
                            _ => out.push(ch as char),
                        }
                        escaped = false;
                    } else if ch == b'\\' {
                        escaped = true;
                    } else if ch == b'"' {
                        break;
                    } else {
                        out.push(ch as char);
                    }
                    idx += 1;
                }
                out
            } else {
                return Err(TypesError::InvalidFormat("Missing content".to_string()));
            }
        };

        // tags
        let tags = {
            if let Some(i) = json.find("\"tags\"") {
                let tail = &json[i + 6..];
                let start_rel = tail
                    .find('[')
                    .ok_or_else(|| TypesError::InvalidFormat("Missing tags array".to_string()))?;
                let start = i + 6 + start_rel;
                let bytes = json.as_bytes();
                let mut pos = start;
                let len = json.len();
                let mut depth = 0usize;
                let mut in_string = false;
                let mut escaped = false;
                let mut end = start;

                while pos < len {
                    let c = bytes[pos];
                    if in_string {
                        if escaped {
                            escaped = false;
                        } else if c == b'\\' {
                            escaped = true;
                        } else if c == b'"' {
                            in_string = false;
                        }
                    } else {
                        match c {
                            b'"' => in_string = true,
                            b'[' => depth += 1,
                            b']' => {
                                if depth == 0 {
                                    return Err(TypesError::InvalidFormat(
                                        "Unbalanced tags array".to_string(),
                                    ));
                                }
                                depth -= 1;
                                if depth == 0 {
                                    end = pos + 1;
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                    pos += 1;
                }

                if end <= start {
                    return Err(TypesError::InvalidFormat("Invalid tags array".to_string()));
                }
                let tags_str = &json[start..end];
                let nt = NostrTags::from_json(tags_str)?;
                nt.0
            } else {
                return Err(TypesError::InvalidFormat("Missing tags".to_string()));
            }
        };

        // created_at
        let created_at = {
            if let Some(i) = json.find("\"created_at\"") {
                let tail = &json[i + 11..];
                // Skip spaces and colon
                let bytes = tail.as_bytes();
                let mut j = 0usize;
                while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b':')
                {
                    j += 1;
                }
                let start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if start == j {
                    return Err(TypesError::InvalidFormat("Invalid created_at".to_string()));
                }
                let num_str = &tail[start..j];
                num_str
                    .parse::<u64>()
                    .map_err(|_| TypesError::InvalidFormat("Invalid created_at".to_string()))?
            } else {
                return Err(TypesError::InvalidFormat("Missing created_at".to_string()));
            }
        };

        if kind > u16::MAX as u32 {
            return Err(TypesError::InvalidFormat("Kind out of range".to_string()));
        }
        Ok(Template {
            kind: kind as u16,
            content,
            tags,
            created_at, // Initialize created_at with the current timestamp when parsing from JSON, adjust according to your requirements
        })
    }

    #[inline(always)]
    fn escape_string(s: &str) -> String {
        let mut result = String::with_capacity(s.len() + 4);
        Self::escape_string_to(&mut result, s);
        result
    }

    #[inline(always)]
    fn escape_string_to(result: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                other => result.push(other),
            }
        }
    }
}

pub struct UnsignedEvent {
    pub pubkey: PublicKey,
    pub kind: Kind,
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

impl UnsignedEvent {
    pub fn new(pubkey: &str, kind: Kind, content: String, tags: Vec<Vec<String>>) -> Result<Self> {
        let pubkey = PublicKey::from_hex(pubkey)?;
        Ok(UnsignedEvent {
            pubkey,
            kind,
            content,
            tags,
        })
    }
}

// Single struct for both signed and unsigned events

#[derive(Clone)]
pub struct Event {
    pub id: EventId,
    pub pubkey: PublicKey,
    pub created_at: Timestamp,
    pub kind: Kind,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

impl Event {
    /// Getter for kind
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// Getter for tags
    pub fn tags(&self) -> &Vec<Vec<String>> {
        &self.tags
    }

    /// Getter for content
    pub fn content(&self) -> &str {
        &self.content
    }

    /// Compute the event ID
    pub fn compute_id(&mut self) -> Result<()> {
        let tags_json = NostrTags(self.tags.clone()).to_json();
        let serialized = format!(
            "[0,\"{}\",{},{},{},\"{}\"]",
            self.pubkey.to_hex(),
            self.created_at,
            self.kind,
            tags_json,
            Self::escape_string(&self.content) // not manual replace
        );

        let mut hasher = Sha256::new();
        hasher.update(serialized.as_bytes());
        let result = hasher.finalize();
        let mut id_bytes = [0u8; 32];
        id_bytes.copy_from_slice(&result);
        self.id = EventId(id_bytes);
        Ok(())
    }

    /// Verify signature
    pub fn verify(&self) -> Result<()> {
        // Check that all required fields are present
        if self.id.0 == [0u8; 32] {
            return Err(TypesError::InvalidFormat("No ID".to_string()));
        }
        if self.pubkey.0 == [0u8; 32] {
            return Err(TypesError::InvalidFormat("No pubkey".to_string()));
        }
        if self.sig.is_empty() {
            return Err(TypesError::InvalidFormat("No signature".to_string()));
        }

        // Verify ID matches
        let mut temp = Event {
            id: EventId([0u8; 32]),
            pubkey: PublicKey(self.pubkey.0),
            created_at: self.created_at,
            kind: self.kind,
            tags: self.tags.clone(),
            content: self.content.clone(),
            sig: String::new(),
        };
        temp.compute_id()?;

        if temp.id.0 != self.id.0 {
            return Err(TypesError::InvalidFormat("ID mismatch".to_string()));
        }

        // Verify signature
        let verifying_key = VerifyingKey::from_bytes(&self.pubkey.0)
            .map_err(|_| TypesError::InvalidFormat("Invalid public key".to_string()))?;
        let signature_bytes = hex::decode(&self.sig)
            .map_err(|_| TypesError::InvalidFormat("Invalid signature hex".to_string()))?;
        let signature = Signature::try_from(signature_bytes.as_slice())
            .map_err(|_| TypesError::InvalidFormat("Invalid signature format".to_string()))?;
        verifying_key
            .verify(&self.id.0, &signature)
            .map_err(|_| TypesError::InvalidFormat("Signature verification failed".to_string()))?;

        Ok(())
    }

    pub fn build_flatbuffer<'a>(
        &self,
        fbb: &mut flatbuffers::FlatBufferBuilder<'a>,
    ) -> flatbuffers::WIPOffset<fb::NostrEvent<'a>> {
        // Required strings
        let id_offset = fbb.create_string(&self.id.to_hex());
        let pubkey_offset = fbb.create_string(&self.pubkey.to_hex());
        let content_offset = fbb.create_string(&self.content);
        let sig_offset = fbb.create_string(&self.sig);

        // Tags -> [StringVec]
        let mut string_vec_offsets = Vec::with_capacity(self.tags.len());
        for tag in &self.tags {
            let tag_strings: Vec<_> = tag.iter().map(|s| fbb.create_string(s)).collect();
            let tag_vector = fbb.create_vector(&tag_strings);
            let string_vec = fb::StringVec::create(
                fbb,
                &fb::StringVecArgs {
                    items: Some(tag_vector),
                },
            );
            string_vec_offsets.push(string_vec);
        }
        let tags_offset = fbb.create_vector(&string_vec_offsets);

        // Assemble NostrEvent
        let args = fb::NostrEventArgs {
            id: Some(id_offset),
            pubkey: Some(pubkey_offset),
            kind: self.kind as u16,
            content: Some(content_offset),
            tags: Some(tags_offset),
            created_at: self.created_at as i32, // schema uses `int`
            sig: Some(sig_offset),
        };

        fb::NostrEvent::create(fbb, &args)
    }

    pub fn as_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        write!(result,
                r#"{{"id":"{}","pubkey":"{}","created_at":{},"kind":{},"tags":{},"content":"{}","sig":"{}"}}"#,
                self.id.to_hex(),
                self.pubkey.to_hex(),
                self.created_at,
                self.kind,
                self.serialize_tags(),
                Self::escape_string(&self.content),
                self.sig
            ).unwrap();

        result
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        // Conservative estimate to avoid reallocations
        200 +
            self.content.len() * 2 +  // Escaping
            self.sig.len() +
            self.calculate_tags_size()
    }

    #[inline(always)]
    fn calculate_tags_size(&self) -> usize {
        self.tags
            .iter()
            .map(|tag| tag.iter().map(|s| s.len() * 2 + 2).sum::<usize>() + tag.len() * 2)
            .sum::<usize>()
            + 20
    }

    #[inline(always)]
    fn serialize_tags(&self) -> String {
        let mut tags_json = String::with_capacity(self.calculate_tags_size());

        tags_json.push('[');
        for (i, tag) in self.tags.iter().enumerate() {
            if i > 0 {
                tags_json.push(',');
            }
            tags_json.push('[');

            for (j, part) in tag.iter().enumerate() {
                if j > 0 {
                    tags_json.push(',');
                }
                tags_json.push('"');
                Self::escape_string_to(&mut tags_json, part);
                tags_json.push('"');
            }

            tags_json.push(']');
        }
        tags_json.push(']');

        tags_json
    }

    #[inline(always)]
    fn escape_string(s: &str) -> String {
        let mut result = String::with_capacity(s.len() + 4);
        Self::escape_string_to(&mut result, s);
        result
    }

    #[inline(always)]
    fn escape_string_to(result: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                other => result.push(other),
            }
        }
    }

    pub fn from_json(json: &str) -> Result<Self> {
        Self::from_json_bytes(json.as_bytes())
    }

    #[inline(always)]
    pub fn from_json_bytes(json_bytes: &[u8]) -> Result<Self> {
        let parser = NostrEventParser::new(json_bytes);
        parser.parse()
    }

    pub fn from_worker_message(bytes: &[u8]) -> Result<Self> {
        let wm = flatbuffers::root::<fb::WorkerMessage>(bytes)?;

        match wm.content_type() {
            fb::Message::NostrEvent => {
                if let Some(ne) = wm.content_as_nostr_event() {
                    // Required fields
                    let id = EventId::from_hex(ne.id())?;
                    let pubkey = PublicKey::from_hex(ne.pubkey())?;
                    let content = ne.content().to_string();
                    let sig = ne.sig().to_string();

                    // Numeric fields
                    let created_at = ne.created_at().max(0) as u64;
                    let kind = ne.kind();

                    // Tags: [StringVec] -> Vec<Vec<String>>
                    let fb_tags = ne.tags();
                    let mut tags: Vec<Vec<String>> = Vec::with_capacity(fb_tags.len());
                    for i in 0..fb_tags.len() {
                        let sv = fb_tags.get(i);
                        let mut tag_vec = Vec::new();
                        if let Some(items) = sv.items() {
                            for s in items.iter() {
                                tag_vec.push(s.to_string());
                            }
                        }
                        tags.push(tag_vec);
                    }

                    Ok(Event {
                        id,
                        pubkey,
                        created_at,
                        kind,
                        tags,
                        content,
                        sig,
                    })
                } else {
                    Err(TypesError::InvalidFormat(
                        "WorkerMessage content missing NostrEvent".to_string(),
                    ))
                }
            }
            _ => Err(TypesError::InvalidFormat(
                "WorkerMessage does not contain a NostrEvent".to_string(),
            )),
        }
    }

    /// Parse an Event from a FlatBuffer-encoded `nostr.fb.NostrEvent`.
    pub fn from_flatbuffer(bytes: &[u8]) -> Result<Self> {
        // Decode FlatBuffer root
        let ne = flatbuffers::root::<fb::NostrEvent>(bytes)?;

        // Required fields (schema enforces presence)
        let id = EventId::from_hex(ne.id())?;
        let pubkey = PublicKey::from_hex(ne.pubkey())?;
        let content = ne.content().to_string();
        let sig = ne.sig().to_string();

        // Numeric fields
        let created_at = ne.created_at().max(0) as u64;
        let kind = ne.kind();

        // Tags: [StringVec] -> Vec<Vec<String>>
        let fb_tags = ne.tags();
        let mut tags: Vec<Vec<String>> = Vec::with_capacity(fb_tags.len());
        for i in 0..fb_tags.len() {
            let sv = fb_tags.get(i);
            let mut tag_vec = Vec::new();
            if let Some(items) = sv.items() {
                for s in items.iter() {
                    tag_vec.push(s.to_string());
                }
            }
            tags.push(tag_vec);
        }

        Ok(Event {
            id,
            pubkey,
            created_at,
            kind,
            tags,
            content,
            sig,
        })
    }
}

// Custom high-performance Nostr event parser
struct NostrEventParser<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> NostrEventParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<Event> {
        self.skip_whitespace();
        self.expect_byte(b'{')?;

        let mut id = EventId([0u8; 32]);
        let mut pubkey = PublicKey([0u8; 32]);
        let mut created_at = 0u64;
        let mut kind = 0u32;
        let mut tags = Vec::new();
        let mut content = String::new();
        let mut sig = String::new();

        // Parse fields in expected order for better performance
        while self.pos < self.bytes.len() {
            self.skip_whitespace();
            if self.peek() == b'}' {
                self.pos += 1;
                break;
            }

            let field_name = self.parse_string()?;
            self.skip_whitespace();
            self.expect_byte(b':')?;
            self.skip_whitespace();

            match field_name {
                "id" => {
                    let hex_str = self.parse_string()?;
                    id = Self::parse_hex_64(hex_str)?;
                }
                "pubkey" => {
                    let hex_str = self.parse_string()?;
                    pubkey = PublicKey::from_hex(hex_str)?;
                }
                "created_at" => {
                    created_at = self.parse_u64()?;
                }
                "kind" => {
                    kind = self.parse_u32()?;
                }
                "tags" => {
                    tags = self.parse_tags()?;
                }
                "content" => {
                    content = self.parse_string()?.to_string();
                }
                "sig" => {
                    sig = self.parse_string()?.to_string();
                }
                _ => {
                    // Skip unknown fields
                    self.skip_value()?;
                }
            }

            self.skip_comma_or_end()?;
        }

        Ok(Event {
            id,
            pubkey,
            created_at,
            kind: kind as u16,
            tags,
            content,
            sig,
        })
    }

    #[inline(always)]
    fn peek(&self) -> u8 {
        self.bytes[self.pos]
    }

    #[inline(always)]
    fn skip_whitespace(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    #[inline(always)]
    fn expect_byte(&mut self, expected: u8) -> Result<()> {
        if self.pos >= self.bytes.len() || self.bytes[self.pos] != expected {
            return Err(TypesError::InvalidFormat("Unexpected byte".to_string()));
        }
        self.pos += 1;
        Ok(())
    }

    #[inline(always)]
    fn parse_string(&mut self) -> Result<&'a str> {
        self.expect_byte(b'"')?;
        let start = self.pos;

        // Find the end quote, handling escaped quotes
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
                    self.pos += 2;
                }
                _ => self.pos += 1,
            }
        }

        Err(TypesError::InvalidFormat("Unterminated string".to_string()))
    }

    #[inline(always)]
    fn parse_u64(&mut self) -> Result<u64> {
        let start = self.pos;
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }

        if start == self.pos {
            return Err(TypesError::InvalidFormat("Expected number".to_string()));
        }

        let num_str = unsafe { std::str::from_utf8_unchecked(&self.bytes[start..self.pos]) };
        num_str
            .parse()
            .map_err(|_| TypesError::InvalidFormat("Invalid number".to_string()))
    }

    #[inline(always)]
    fn parse_u32(&mut self) -> Result<u32> {
        self.parse_u64().map(|n| n as u32)
    }

    #[inline(always)]
    fn parse_hex_64(hex_str: &str) -> Result<EventId> {
        if hex_str.len() != 64 {
            return Err(TypesError::InvalidFormat(
                "Hex string must be 64 characters".to_string(),
            ));
        }

        let mut bytes = [0u8; 32];
        hex::decode_to_slice(hex_str, &mut bytes)
            .map_err(|_| TypesError::InvalidFormat("Invalid hex".to_string()))?;
        Ok(EventId(bytes))
    }

    #[inline(always)]
    fn parse_tags(&mut self) -> Result<Vec<Vec<String>>> {
        self.expect_byte(b'[')?;
        let mut tags = Vec::new();

        while self.pos < self.bytes.len() {
            self.skip_whitespace();
            if self.peek() == b']' {
                self.pos += 1;
                break;
            }

            tags.push(self.parse_tag_array()?);
            self.skip_comma_or_end()?;
        }

        Ok(tags)
    }

    #[inline(always)]
    fn parse_tag_array(&mut self) -> Result<Vec<String>> {
        self.expect_byte(b'[')?;
        let mut tag = Vec::new();

        while self.pos < self.bytes.len() {
            self.skip_whitespace();
            if self.peek() == b']' {
                self.pos += 1;
                break;
            }

            let value = self.parse_string()?.to_string();
            tag.push(value);
            self.skip_comma_or_end()?;
        }

        Ok(tag)
    }

    #[inline(always)]
    fn skip_value(&mut self) -> Result<()> {
        match self.peek() {
            b'"' => {
                self.parse_string()?;
            }
            b'[' => {
                self.skip_array()?;
            }
            b'{' => {
                self.skip_object()?;
            }
            b't' | b'f' => {
                self.skip_bool()?;
            }
            b'n' => {
                self.skip_null()?;
            }
            _ => {
                self.skip_number()?;
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn skip_array(&mut self) -> Result<()> {
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
    fn skip_object(&mut self) -> Result<()> {
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
    fn skip_bool(&mut self) -> Result<()> {
        if self.bytes[self.pos..].starts_with(b"true") {
            self.pos += 4;
        } else if self.bytes[self.pos..].starts_with(b"false") {
            self.pos += 5;
        } else {
            return Err(TypesError::InvalidFormat("Invalid boolean".to_string()));
        }
        Ok(())
    }

    #[inline(always)]
    fn skip_null(&mut self) -> Result<()> {
        if self.bytes[self.pos..].starts_with(b"null") {
            self.pos += 4;
        } else {
            return Err(TypesError::InvalidFormat("Invalid null".to_string()));
        }
        Ok(())
    }

    #[inline(always)]
    fn skip_number(&mut self) -> Result<()> {
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
    fn skip_comma_or_end(&mut self) -> Result<()> {
        self.skip_whitespace();
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b',' {
            self.pos += 1;
        }
        Ok(())
    }
}

// Minimal filter for queries
#[derive(Clone)]
pub struct Filter {
    pub ids: Option<Vec<EventId>>,
    pub authors: Option<Vec<PublicKey>>,
    pub kinds: Option<Vec<Kind>>,
    pub e_tags: Option<Vec<String>>,
    pub p_tags: Option<Vec<String>>,
    pub d_tags: Option<Vec<String>>,
    pub a_tags: Option<Vec<String>>,
    pub tags: Option<HashMap<String, Vec<String>>>,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub limit: Option<usize>,
    pub search: Option<String>,
}

impl Filter {
    pub fn new() -> Self {
        Filter {
            ids: None,
            authors: None,
            kinds: None,
            e_tags: None,
            p_tags: None,
            d_tags: None,
            a_tags: None,
            tags: None,
            since: None,
            until: None,
            limit: None,
            search: None,
        }
    }

    pub fn id(mut self, id: EventId) -> Self {
        self.ids.get_or_insert(vec![]).push(id);
        self
    }

    pub fn author(mut self, author: PublicKey) -> Self {
        self.authors.get_or_insert(vec![]).push(author);
        self
    }

    pub fn kind(mut self, kind: Kind) -> Self {
        self.kinds.get_or_insert(vec![]).push(kind);
        self
    }

    pub fn custom_tag(mut self, tag_name: &str, values: Vec<String>) -> Self {
        match tag_name {
            "e" => self.e_tags = Some(values),
            "p" => self.p_tags = Some(values),
            "d" => self.d_tags = Some(values),
            "a" => self.a_tags = Some(values),
            _ => {} // Ignore others for now
        }
        self
    }

    pub fn as_json(&self) -> String {
        // Pre-calculate approximate size to avoid reallocations
        let capacity = self.calculate_json_size();
        let mut json = String::with_capacity(capacity);

        json.push('{');
        let mut first_field = true;

        macro_rules! add_optional_field {
            ($field:expr, $name:expr, $formatter:expr) => {
                if let Some(ref value) = $field {
                    if !first_field {
                        json.push(',');
                    }
                    first_field = false;
                    json.push('"');
                    json.push_str($name);
                    json.push_str("\":[");
                    json.push_str(&$formatter(value));
                    json.push(']');
                }
            };
        }

        // Handle array fields
        add_optional_field!(self.ids, "ids", |ids: &Vec<EventId>| {
            ids.iter()
                .map(|id| format!(r#""{}""#, id.to_hex()))
                .collect::<Vec<_>>()
                .join(",")
        });

        add_optional_field!(self.authors, "authors", |authors: &Vec<PublicKey>| {
            authors
                .iter()
                .map(|author| format!(r#""{}""#, author.to_hex()))
                .collect::<Vec<_>>()
                .join(",")
        });

        add_optional_field!(self.kinds, "kinds", |kinds: &Vec<Kind>| {
            kinds
                .iter()
                .map(|k| k.to_string())
                .collect::<Vec<_>>()
                .join(",")
        });

        // Handle tag fields with proper escaping
        add_optional_field!(self.e_tags, "#e", |tags: &Vec<String>| {
            Self::format_tags(tags)
        });

        add_optional_field!(self.p_tags, "#p", |tags: &Vec<String>| {
            Self::format_tags(tags)
        });

        add_optional_field!(self.d_tags, "#d", |tags: &Vec<String>| {
            Self::format_tags(tags)
        });

        add_optional_field!(self.a_tags, "#a", |tags: &Vec<String>| {
            Self::format_tags(tags)
        });

        if let Some(ref tags_map) = self.tags {
            for (key, values) in tags_map {
                add_optional_field!(Some(values), &format!("#{}", key), |tags: &Vec<String>| {
                    Self::format_tags(tags)
                });
            }
        }

        // Handle scalar fields
        if let Some(since) = self.since {
            if !first_field {
                json.push(',');
            }
            first_field = false;
            json.push_str(&format!(r#""since":{}"#, since));
        }

        if let Some(until) = self.until {
            if !first_field {
                json.push(',');
            }
            first_field = false;
            json.push_str(&format!(r#""until":{}"#, until));
        }

        if let Some(limit) = self.limit {
            if !first_field {
                json.push(',');
            }
            first_field = false;
            json.push_str(&format!(r#""limit":{}"#, limit));
        }

        if let Some(ref search) = self.search {
            if !search.is_empty() {
                if !first_field {
                    json.push(',');
                }
                first_field = false;
                json.push_str(&format!(r#""search":"{}""#, Self::escape_string(search)));
            }
        }

        json.push('}');
        json
    }

    #[inline(always)]
    fn calculate_json_size(&self) -> usize {
        let mut size = 2; // {}

        if let Some(ref ids) = self.ids {
            size += 10 + ids.len() * 70; // "ids":[] + hex strings
        }
        if let Some(ref authors) = self.authors {
            size += 15 + authors.len() * 70; // "authors":[] + hex strings
        }
        if let Some(ref kinds) = self.kinds {
            size += 10 + kinds.len() * 10; // "kinds":[] + numbers
        }
        if let Some(ref e_tags) = self.e_tags {
            size += 8 + Self::calculate_tags_size(e_tags); // "#e":[]
        }
        if let Some(ref p_tags) = self.p_tags {
            size += 8 + Self::calculate_tags_size(p_tags); // "#p":[]
        }
        if let Some(ref d_tags) = self.d_tags {
            size += 8 + Self::calculate_tags_size(d_tags); // "#d":[]
        }
        if let Some(ref a_tags) = self.a_tags {
            size += 8 + Self::calculate_tags_size(a_tags); // "#a":[]
        }
        if self.since.is_some() {
            size += 15;
        } // "since":number
        if self.until.is_some() {
            size += 15;
        } // "until":number
        if self.limit.is_some() {
            size += 15;
        } // "limit":number
        if let Some(ref search) = self.search {
            size += 12 + search.len() * 2; // "search":"" + escaping
        }

        size
    }

    #[inline(always)]
    fn calculate_tags_size(tags: &Vec<String>) -> usize {
        tags.iter().map(|tag| tag.len() * 2 + 4).sum::<usize>() + tags.len() * 3
    }

    #[inline(always)]
    fn format_tags(tags: &Vec<String>) -> String {
        tags.iter()
            .map(|tag| format!(r#""{}""#, Self::escape_string(tag)))
            .collect::<Vec<_>>()
            .join(",")
    }

    #[inline(always)]
    fn escape_string(s: &str) -> String {
        if !s.contains('\\') && !s.contains('"') {
            s.to_string()
        } else {
            s.replace('\\', "\\\\").replace('"', "\\\"")
        }
    }
}

// Kind constants - just functions, not impl on u64
pub type Kind = u16;
pub const METADATA: Kind = 0;
pub const TEXT_NOTE: Kind = 1;
pub const CONTACT_LIST: Kind = 3;
pub const ENCRYPTED_DIRECT_MESSAGE: Kind = 4;
pub const DELETION: Kind = 5;
pub const REPOST: Kind = 6;
pub const REACTION: Kind = 7;
pub const RELAY_LIST: Kind = 10002;

// Timestamp helper
pub fn timestamp_now() -> Timestamp {
    (js_sys::Date::now() / 1000.0) as u64
}

// ============================================================================
// NIP-19 (Bech32) - Stubbed out since bech32 is not in dependencies
// ============================================================================

pub mod nips {
    pub mod nip19 {
        use super::super::*;

        pub enum Nip19 {
            Pubkey(PublicKey),
            EventId(EventId),
            Profile(Nip19Profile),
            Event(Nip19Event),
            Coordinate(Nip19Coordinate),
        }

        pub struct Nip19Profile {
            pub public_key: PublicKey,
            pub relays: Vec<String>,
        }

        pub struct Nip19Event {
            pub event_id: EventId,
            pub author: Option<PublicKey>,
            pub relays: Vec<String>,
        }

        pub struct Nip19Coordinate {
            pub identifier: String,
            pub public_key: PublicKey,
            pub kind: u16,
            pub relays: Vec<String>,
        }

        pub trait FromBech32 {
            fn from_bech32(s: &str) -> Result<Self>
            where
                Self: Sized;
        }

        impl FromBech32 for Nip19 {
            fn from_bech32(s: &str) -> Result<Self> {
                // Bech32 character set
                const CHARSET: &[u8] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";

                // Convert bech32 character to value
                fn decode_char(c: u8) -> Option<u8> {
                    CHARSET.iter().position(|&x| x == c).map(|p| p as u8)
                }

                // Convert between bit groups
                fn convert_bits(data: &[u8], from_bits: u32, to_bits: u32) -> Option<Vec<u8>> {
                    let mut acc = 0u32;
                    let mut bits = 0u32;
                    let mut ret = Vec::new();
                    let maxv = (1 << to_bits) - 1;

                    for &value in data {
                        if (value as u32) >> from_bits != 0 {
                            return None;
                        }
                        acc = (acc << from_bits) | (value as u32);
                        bits += from_bits;

                        while bits >= to_bits {
                            bits -= to_bits;
                            ret.push(((acc >> bits) & maxv) as u8);
                        }
                    }

                    if bits >= from_bits || ((acc << (to_bits - bits)) & maxv) != 0 {
                        return None;
                    }

                    Some(ret)
                }

                // Parse TLV data for nprofile and nevent
                fn parse_tlv(
                    mut data: Vec<u8>,
                ) -> Result<(
                    Option<Vec<u8>>,  // Special field (can be pubkey, event_id, or identifier)
                    Option<[u8; 32]>, // Author field
                    Option<u16>,      // Kind field
                    Vec<String>,      // Relays
                )> {
                    let mut special = None;
                    let mut author = None;
                    let mut kind = None;
                    let mut relays = Vec::new();

                    while !data.is_empty() {
                        if data.len() < 2 {
                            break;
                        }

                        let t = data[0];
                        let l = data[1] as usize;

                        if data.len() < 2 + l {
                            return Err(TypesError::InvalidFormat("Invalid TLV data".to_string()));
                        }

                        let value = &data[2..2 + l];

                        match t {
                            0 => {
                                // Special field (can be 32 bytes or variable length for identifier)
                                if special.is_none() {
                                    special = Some(value.to_vec());
                                }
                            }
                            1 => {
                                // Relay
                                if let Ok(relay) = String::from_utf8(value.to_vec()) {
                                    relays.push(relay);
                                }
                            }
                            2 => {
                                // Author
                                if l == 32 && author.is_none() {
                                    let mut arr = [0u8; 32];
                                    arr.copy_from_slice(value);
                                    author = Some(arr);
                                }
                            }
                            3 => {
                                // Kind
                                if l == 4 && kind.is_none() {
                                    let bytes: [u8; 4] = value.try_into().map_err(|_| {
                                        TypesError::InvalidFormat("Invalid kind".to_string())
                                    })?;
                                    kind = Some(u32::from_be_bytes(bytes) as u16);
                                }
                            }
                            _ => {} // Skip unknown TLV types
                        }

                        data.drain(..2 + l);
                    }

                    Ok((special, author, kind, relays))
                }

                // Convert to lowercase
                let s = s.to_lowercase();

                // Find separator '1'
                let sep_pos = s.rfind('1').ok_or_else(|| {
                    TypesError::InvalidFormat("Missing separator '1'".to_string())
                })?;

                if sep_pos == 0 || sep_pos == s.len() - 1 {
                    return Err(TypesError::InvalidFormat(
                        "Invalid bech32 format".to_string(),
                    ));
                }

                // Split HRP and data
                let hrp = &s[..sep_pos];
                let data_str = &s[sep_pos + 1..];

                // Decode data characters
                let mut data = Vec::new();
                for c in data_str.bytes() {
                    match decode_char(c) {
                        Some(val) => data.push(val),
                        None => {
                            return Err(TypesError::InvalidFormat(format!(
                                "Invalid bech32 character: {}",
                                c as char
                            )))
                        }
                    }
                }

                // Verify minimum length for checksum
                if data.len() < 6 {
                    return Err(TypesError::InvalidFormat("Data too short".to_string()));
                }

                // Remove checksum (last 6 characters) - simplified, not verifying
                data.truncate(data.len() - 6);

                // Convert 5-bit groups to 8-bit bytes
                let bytes = convert_bits(&data, 5, 8).ok_or_else(|| {
                    TypesError::InvalidFormat("Failed to convert bits".to_string())
                })?;

                // Parse based on HRP
                match hrp {
                    "npub" => {
                        if bytes.len() != 32 {
                            return Err(TypesError::InvalidFormat(format!(
                                "Invalid npub length: {}",
                                bytes.len()
                            )));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        Ok(Nip19::Pubkey(PublicKey(arr)))
                    }
                    "note" => {
                        if bytes.len() != 32 {
                            return Err(TypesError::InvalidFormat(format!(
                                "Invalid note length: {}",
                                bytes.len()
                            )));
                        }
                        let mut arr = [0u8; 32];
                        arr.copy_from_slice(&bytes);
                        Ok(Nip19::EventId(EventId(arr)))
                    }
                    "nprofile" => {
                        let (special, _, _, relays) = parse_tlv(bytes)?;

                        let special_bytes = special.ok_or_else(|| {
                            TypesError::InvalidFormat("Missing public key in nprofile".to_string())
                        })?;

                        if special_bytes.len() != 32 {
                            return Err(TypesError::InvalidFormat(
                                "Invalid public key length in nprofile".to_string(),
                            ));
                        }

                        let mut public_key = [0u8; 32];
                        public_key.copy_from_slice(&special_bytes);

                        Ok(Nip19::Profile(Nip19Profile {
                            public_key: PublicKey(public_key),
                            relays,
                        }))
                    }
                    "nevent" => {
                        let (special, author, _kind, relays) = parse_tlv(bytes)?;

                        let special_bytes = special.ok_or_else(|| {
                            TypesError::InvalidFormat("Missing event ID in nevent".to_string())
                        })?;

                        if special_bytes.len() != 32 {
                            return Err(TypesError::InvalidFormat(
                                "Invalid event ID length in nevent".to_string(),
                            ));
                        }

                        let mut event_id = [0u8; 32];
                        event_id.copy_from_slice(&special_bytes);

                        Ok(Nip19::Event(Nip19Event {
                            event_id: EventId(event_id),
                            author: author.map(PublicKey),
                            relays,
                        }))
                    }
                    "naddr" => {
                        let (special, author, kind, relays) = parse_tlv(bytes)?;

                        let identifier = special
                            .ok_or_else(|| {
                                TypesError::InvalidFormat("Missing identifier in naddr".to_string())
                            })
                            .and_then(|bytes| {
                                String::from_utf8(bytes).map_err(|_| {
                                    TypesError::InvalidFormat(
                                        "Invalid identifier in naddr".to_string(),
                                    )
                                })
                            })?;

                        let public_key = author.ok_or_else(|| {
                            TypesError::InvalidFormat("Missing public key in naddr".to_string())
                        })?;

                        let kind = kind.ok_or_else(|| {
                            TypesError::InvalidFormat("Missing kind in naddr".to_string())
                        })?;

                        Ok(Nip19::Coordinate(Nip19Coordinate {
                            identifier,
                            public_key: PublicKey(public_key),
                            kind,
                            relays,
                        }))
                    }
                    _ => Err(TypesError::InvalidFormat(format!(
                        "Unknown bech32 prefix: {}",
                        hrp
                    ))),
                }
            }
        }
    }

    // NIP-04 encryption if needed
    pub mod nip04 {
        use super::super::*;

        pub fn encrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }

        pub fn decrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }
    }

    // NIP-44 encryption if needed
    pub mod nip44 {
        use super::super::*;

        pub fn encrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }

        pub fn decrypt(
            _secret_key: &SecretKey,
            _pubkey: &PublicKey,
            _text: &str,
        ) -> Result<String> {
            // Implement only if you actually use it
            Ok(String::new())
        }
    }
}

enum NostrTagsParserData<'a> {
    Borrowed(&'a [u8]),
    Owned(Vec<u8>),
}

struct NostrTagsParser<'a> {
    data: NostrTagsParserData<'a>,
}

impl<'a> NostrTagsParser<'a> {
    #[inline(always)]
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            data: NostrTagsParserData::Borrowed(bytes),
        }
    }

    #[inline(always)]
    fn parse(mut self) -> Result<NostrTags> {
        // Get the bytes to parse
        let bytes = match &self.data {
            NostrTagsParserData::Borrowed(b) => *b,
            NostrTagsParserData::Owned(v) => v.as_slice(),
        };

        // Handle escaped JSON if needed
        let mut parser = if let Some(unescaped) = BaseJsonParser::unescape_if_needed(bytes)? {
            // Use the unescaped data
            self.data = NostrTagsParserData::Owned(unescaped);
            match &self.data {
                NostrTagsParserData::Owned(v) => BaseJsonParser::new(v.as_slice()),
                _ => unreachable!(),
            }
        } else {
            BaseJsonParser::new(bytes)
        };

        parser.skip_whitespace();
        parser.expect_byte(b'[')?;

        let mut tags = Vec::new();

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }

            let tag = self.parse_tag(&mut parser)?;
            tags.push(tag);
            parser.skip_comma_or_end()?;
        }

        Ok(NostrTags(tags))
    }

    #[inline(always)]
    fn parse_tag(&self, parser: &mut BaseJsonParser) -> Result<Vec<String>> {
        parser.expect_byte(b'[')?;
        let mut tag = Vec::new();

        while parser.pos < parser.bytes.len() {
            parser.skip_whitespace();
            if parser.peek() == b']' {
                parser.pos += 1;
                break;
            }

            let value = parser.parse_string_unescaped()?;
            tag.push(value);
            parser.skip_comma_or_end()?;
        }

        Ok(tag)
    }
}

/// NostrTags represents a list of Nostr tags: Vec<Vec<String>>
/// Using newtype pattern to allow inherent implementations
#[derive(Debug, Clone, PartialEq)]
pub struct NostrTags(pub Vec<Vec<String>>);

impl NostrTags {
    /// Parse NostrTags from JSON string
    pub fn from_json(json: &str) -> Result<Self> {
        let parser = NostrTagsParser::new(json.as_bytes());
        parser.parse()
    }

    /// Serialize NostrTags to JSON string
    pub fn to_json(&self) -> String {
        let mut result = String::with_capacity(self.calculate_json_size());

        result.push('[');
        for (i, tag) in self.0.iter().enumerate() {
            if i > 0 {
                result.push(',');
            }
            result.push('[');

            for (j, part) in tag.iter().enumerate() {
                if j > 0 {
                    result.push(',');
                }
                result.push('"');
                Self::escape_string_to(&mut result, part);
                result.push('"');
            }

            result.push(']');
        }
        result.push(']');

        result
    }

    #[inline(always)]
    pub fn calculate_json_size(&self) -> usize {
        let mut size = 2; // []

        for tag in &self.0 {
            size += 2; // []
            for part in tag {
                size += part.len() * 2 + 4; // Escaped string + quotes + comma
            }
            if !tag.is_empty() {
                size -= 1; // Remove last comma
            }
        }

        size
    }

    #[inline(always)]
    fn escape_string_to(result: &mut String, s: &str) {
        for ch in s.chars() {
            match ch {
                '\\' => result.push_str("\\\\"),
                '"' => result.push_str("\\\""),
                '\n' => result.push_str("\\n"),
                '\r' => result.push_str("\\r"),
                '\t' => result.push_str("\\t"),
                other => result.push(other),
            }
        }
    }
}

// Re-export for compatibility
pub use nips::*;
