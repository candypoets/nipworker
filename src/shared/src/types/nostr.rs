use crate::{generated::nostr::fb, types::TypesError, utils::BaseJsonParser};
use std::{collections::HashMap, fmt::Write};

type Result<T> = std::result::Result<T, TypesError>;

// ============================================================================
// Basic Types - Simple string/byte wrappers without crypto
// ============================================================================
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
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
	#[cfg(feature = "crypto")]
	pub fn new(secret_key: SecretKey) -> Self {
		let public_key = secret_key.public_key_from_secret();
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
		let _secret_key = SecretKey::from_hex(nsec)?;
		#[cfg(feature = "crypto")]
		{
			Ok(Self::new(_secret_key))
		}
		#[cfg(not(feature = "crypto"))]
		{
			Err(TypesError::InvalidFormat(
				"SecretKey::new requires 'crypto' feature".to_string(),
			))
		}
	}

	#[cfg(feature = "crypto")]
	pub fn generate() -> Self {
		use k256::elliptic_curve::rand_core::OsRng;
		use k256::schnorr::SigningKey;

		let signing_key = SigningKey::random(&mut OsRng);
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

impl SecretKey {
	#[cfg(feature = "crypto")]
	pub fn public_key_from_secret(&self) -> PublicKey {
		use k256::schnorr::SigningKey;

		let signing_key = SigningKey::from_bytes(&self.0)
			.expect("Secret key must be valid 32-byte scalar");
		let verifying_key = signing_key.verifying_key();
		PublicKey(verifying_key.to_bytes().into())
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
	pub created_at: Timestamp,
}

impl Template {
	pub fn new(kind: Kind, content: String, tags: Vec<Vec<String>>) -> Self {
		Template {
			kind,
			content,
			tags,
			created_at: timestamp_now(),
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

	/// Serialize Template to a compact JSON string
	pub fn to_json(&self) -> String {
		let tags_json = NostrTags(self.tags.clone()).to_json();
		let mut result = String::with_capacity(32 + self.content.len() * 2 + tags_json.len() + 20);
		use core::fmt::Write;
		write!(
			result,
			"{{\"kind\":{},\"content\":\"{}\",\"tags\":{},\"created_at\":{}}}",
			self.kind,
			Self::escape_string(&self.content),
			tags_json,
			self.created_at
		)
		.unwrap();
		result
	}

	/// Parse Template from JSON
	pub fn from_json(json: &str) -> Result<Self> {
		// kind
		let kind = {
			if let Some(i) = json.find("\"kind\"") {
				let tail = &json[i + 6..];
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
				let mut raw_bytes = Vec::new();
				let mut escaped = false;
				while idx < tail.len() {
					let ch = bytes[idx];
					if escaped {
						match ch {
							b'"' => raw_bytes.push(b'"'),
							b'\\' => raw_bytes.push(b'\\'),
							b'n' => raw_bytes.push(b'\n'),
							b'r' => raw_bytes.push(b'\r'),
							b't' => raw_bytes.push(b'\t'),
							_ => raw_bytes.push(ch),
						}
						escaped = false;
					} else if ch == b'\\' {
						escaped = true;
					} else if ch == b'"' {
						break;
					} else {
						raw_bytes.push(ch);
					}
					idx += 1;
				}
				String::from_utf8(raw_bytes)
					.map_err(|_| TypesError::InvalidFormat("Invalid UTF-8 in content".to_string()))?
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
				let tail = &json[i + 12..];
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
			created_at,
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

		fb::NostrEvent::create(
			fbb,
			&fb::NostrEventArgs {
				id: Some(id_offset),
				pubkey: Some(pubkey_offset),
				created_at: self.created_at as i32,
				kind: self.kind,
				tags: Some(tags_offset),
				content: Some(content_offset),
				sig: Some(sig_offset),
			},
		)
	}

	pub fn from_flatbuffer(fb_event: &fb::NostrEvent) -> Result<Self> {
		let id_str = fb_event.id();
		if id_str.is_empty() {
			return Err(TypesError::InvalidFormat("Missing event ID".to_string()));
		}
		let id = EventId::from_hex(id_str)?;
		
		let pubkey_str = fb_event.pubkey();
		if pubkey_str.is_empty() {
			return Err(TypesError::InvalidFormat("Missing pubkey".to_string()));
		}
		let pubkey = PublicKey::from_hex(pubkey_str)?;

		let mut tags = Vec::new();
		let fb_tags = fb_event.tags();
		for i in 0..fb_tags.len() {
			let tag_vec = fb_tags.get(i);
			if let Some(items) = tag_vec.items() {
				let tag: Vec<String> = items.iter().map(|s| s.to_string()).collect();
				tags.push(tag);
			}
		}

		Ok(Event {
			id,
			pubkey,
			created_at: fb_event.created_at() as u64,
			kind: fb_event.kind(),
			tags,
			content: fb_event.content().to_string(),
			sig: fb_event.sig().to_string(),
		})
	}

	pub fn to_json(&self) -> String {
		let tags_json = NostrTags(self.tags.clone()).to_json();
		let mut result =
			String::with_capacity(64 + self.content.len() * 2 + tags_json.len() + 20);
		use core::fmt::Write;
		write!(
			result,
			r#"{{"id":"{}","pubkey":"{}","created_at":{},"kind":{},"tags":{},"content":"{}","sig":"{}"}}"#,
			self.id.to_hex(),
			self.pubkey.to_hex(),
			self.created_at,
			self.kind,
			tags_json,
			Self::escape_string(&self.content),
			self.sig
		)
		.unwrap();
		result
	}

	pub fn from_json(json: &str) -> Result<Self> {
		// id
		let id = {
			if let Some(i) = json.find("\"id\"") {
				let tail = &json[i + 4..];
				if let Some(q1) = tail.find('"') {
					let id_start = q1 + 1;
					if let Some(q2) = tail[id_start..].find('"') {
						let id_end = id_start + q2;
						EventId::from_hex(&tail[id_start..id_end])?
					} else {
						return Err(TypesError::InvalidFormat("Missing id value".to_string()));
					}
				} else {
					return Err(TypesError::InvalidFormat("Invalid id format".to_string()));
				}
			} else {
				return Err(TypesError::InvalidFormat("Missing id".to_string()));
			}
		};

		// pubkey
		let pubkey = {
			if let Some(i) = json.find("\"pubkey\"") {
				let tail = &json[i + 8..];
				if let Some(q1) = tail.find('"') {
					let pk_start = q1 + 1;
					if let Some(q2) = tail[pk_start..].find('"') {
						let pk_end = pk_start + q2;
						PublicKey::from_hex(&tail[pk_start..pk_end])?
					} else {
						return Err(TypesError::InvalidFormat("Missing pubkey value".to_string()));
					}
				} else {
					return Err(TypesError::InvalidFormat("Invalid pubkey format".to_string()));
				}
			} else {
				return Err(TypesError::InvalidFormat("Missing pubkey".to_string()));
			}
		};

		// created_at
		let created_at = {
			if let Some(i) = json.find("\"created_at\"") {
				let tail = &json[i + 12..];
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

		// kind
		let kind = {
			if let Some(i) = json.find("\"kind\"") {
				let tail = &json[i + 6..];
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

		// content
		let content = {
			if let Some(i) = json.find("\"content\"") {
				let tail = &json[i + 9..];
				let qpos = tail.find('"').ok_or_else(|| {
					TypesError::InvalidFormat("Missing content string".to_string())
				})?;
				let bytes = tail.as_bytes();
				let mut idx = qpos + 1;
				let mut raw_bytes = Vec::new();
				let mut escaped = false;
				while idx < tail.len() {
					let ch = bytes[idx];
					if escaped {
						match ch {
							b'"' => raw_bytes.push(b'"'),
							b'\\' => raw_bytes.push(b'\\'),
							b'n' => raw_bytes.push(b'\n'),
							b'r' => raw_bytes.push(b'\r'),
							b't' => raw_bytes.push(b'\t'),
							_ => raw_bytes.push(ch),
						}
						escaped = false;
					} else if ch == b'\\' {
						escaped = true;
					} else if ch == b'"' {
						break;
					} else {
						raw_bytes.push(ch);
					}
					idx += 1;
				}
				String::from_utf8(raw_bytes)
					.map_err(|_| TypesError::InvalidFormat("Invalid UTF-8 in content".to_string()))?
			} else {
				return Err(TypesError::InvalidFormat("Missing content".to_string()));
			}
		};

		// sig
		let sig = {
			if let Some(i) = json.find("\"sig\"") {
				let tail = &json[i + 5..];
				if let Some(q1) = tail.find('"') {
					let sig_start = q1 + 1;
					if let Some(q2) = tail[sig_start..].find('"') {
						let sig_end = sig_start + q2;
						tail[sig_start..sig_end].to_string()
					} else {
						return Err(TypesError::InvalidFormat("Missing sig value".to_string()));
					}
				} else {
					return Err(TypesError::InvalidFormat("Invalid sig format".to_string()));
				}
			} else {
				String::new()
			}
		};

		if kind > u16::MAX as u32 {
			return Err(TypesError::InvalidFormat("Kind out of range".to_string()));
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

// ============================================================================
// Filter
// ============================================================================

#[derive(Clone, Debug)]
pub struct Filter {
	pub ids: Option<Vec<EventId>>,
	pub authors: Option<Vec<PublicKey>>,
	pub kinds: Option<Vec<Kind>>,
	pub since: Option<Timestamp>,
	pub until: Option<Timestamp>,
	pub limit: Option<u32>,
	pub tags: HashMap<String, Vec<String>>,
	pub search: Option<String>,
	pub e_tags: Option<Vec<String>>,
	pub p_tags: Option<Vec<String>>,
	pub d_tags: Option<Vec<String>>,
	pub a_tags: Option<Vec<String>>,
}

impl Filter {
	pub fn new() -> Self {
		Filter {
			ids: None,
			authors: None,
			kinds: None,
			since: None,
			until: None,
			limit: None,
			tags: HashMap::new(),
			search: None,
			e_tags: None,
			p_tags: None,
			d_tags: None,
			a_tags: None,
		}
	}
}

// ============================================================================
// Relay Message
// ============================================================================

pub struct RelayMessage {
	pub message_type: String,
	pub data: String,
}

impl RelayMessage {
	pub fn new(message_type: String, data: String) -> Self {
		RelayMessage {
			message_type,
			data,
		}
	}
}

// ============================================================================
// Kind constants
// ============================================================================

pub type Kind = u16;

pub const METADATA: Kind = 0;
pub const TEXT_NOTE: Kind = 1;
pub const RECOMMEND_SERVER: Kind = 2;
pub const CONTACT_LIST: Kind = 3;
pub const ENCRYPTED_DIRECT_MESSAGE: Kind = 4;
pub const EVENT_DELETION: Kind = 5;
pub const REPOST: Kind = 6;
pub const REACTION: Kind = 7;
pub const BADGE_AWARD: Kind = 8;
pub const GENERIC_REPOST: Kind = 16;
pub const CHANNEL_CREATE: Kind = 40;
pub const CHANNEL_METADATA: Kind = 41;
pub const CHANNEL_MESSAGE: Kind = 42;
pub const CHANNEL_HIDE_MESSAGE: Kind = 43;
pub const CHANNEL_MUTE_USER: Kind = 44;
pub const REPORTING: Kind = 1984;
pub const ZAP_REQUEST: Kind = 9734;
pub const ZAP: Kind = 9735;
pub const ZAP_GOAL: Kind = 9740;
pub const MUTE_LIST: Kind = 10000;
pub const PIN_LIST: Kind = 10001;
pub const RELAY_LIST: Kind = 10002;

// Alias for compatibility
pub const DELETION: Kind = EVENT_DELETION;

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
