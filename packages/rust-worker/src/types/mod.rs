//! Types module for Nutscash Nostr
//!
//! This module contains all the type definitions used throughout the Nostr implementation,
//! including event types, request types, proof types, and communication types.

pub mod network;
pub mod proof;
pub mod thread;

// Re-export module types
pub use network::EOSE;
pub use proof::Proof;
pub use thread::*;

// Re-export nostr types for convenience
pub use nostr::{
    Alphabet, Event, EventId, Filter, Kind, PublicKey, SingleLetterTag, Tag, Timestamp,
};

// Import parsed types from parser module
use crate::parser::{
    Kind0Parsed, Kind10002Parsed, Kind10019Parsed, Kind17375Parsed, Kind17Parsed, Kind1Parsed,
    Kind3Parsed, Kind4Parsed, Kind6Parsed, Kind7374Parsed, Kind7375Parsed, Kind7376Parsed,
    Kind7Parsed, Kind9321Parsed, Kind9735Parsed,
};

use nostr::{EventBuilder, UnsignedEvent};
use serde::{Deserialize, Serialize};

use wasm_bindgen::prelude::*;

use crate::types::network::Request;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventTemplate {
    pub kind: u64,
    pub content: String,
    pub tags: Vec<Vec<String>>,
}

impl EventTemplate {
    pub fn to_unsigned_event(&self, pubkey: PublicKey) -> Result<UnsignedEvent, String> {
        let kind = Kind::from(self.kind);

        let mut tags = Vec::new();
        for tag_vec in &self.tags {
            if !tag_vec.is_empty() {
                let tag = Tag::parse(tag_vec.clone()).map_err(|e| format!("Invalid tag: {}", e))?;
                tags.push(tag);
            }
        }

        let event_builder =
            EventBuilder::new(kind, &self.content, tags).custom_created_at(nostr::Timestamp::now());

        Ok(event_builder.to_unsigned_event(pubkey))
    }
}

/// SerializableParsedEvent represents a ParsedEvent with hex string fields for msgpack serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SerializableParsedEvent {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: u32,
    pub tags: Vec<Vec<String>>,
    pub content: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<serde_json::Value>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<Request>>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub relays: Vec<String>,
}

impl From<ParsedEvent> for SerializableParsedEvent {
    fn from(parsed_event: ParsedEvent) -> Self {
        Self {
            id: parsed_event.event.id.to_hex(),
            pubkey: parsed_event.event.pubkey.to_hex(),
            created_at: parsed_event.event.created_at.as_i64(),
            kind: parsed_event.event.kind.as_u32(),
            tags: parsed_event.event.tags.iter().map(|t| t.as_vec()).collect(),
            content: parsed_event.event.content.clone(),
            parsed: parsed_event
                .parsed
                .and_then(|p| serde_json::to_value(p).ok()),
            requests: parsed_event.requests,
            relays: parsed_event.relays,
        }
    }
}

/// Strongly typed parsed data for different event kinds
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data")]
pub enum ParsedData {
    #[serde(rename = "0")]
    Kind0(Kind0Parsed),
    #[serde(rename = "1")]
    Kind1(Kind1Parsed),
    #[serde(rename = "3")]
    Kind3(Kind3Parsed),
    #[serde(rename = "4")]
    Kind4(Kind4Parsed),
    #[serde(rename = "6")]
    Kind6(Kind6Parsed),
    #[serde(rename = "7")]
    Kind7(Kind7Parsed),
    #[serde(rename = "17")]
    Kind17(Kind17Parsed),
    #[serde(rename = "7374")]
    Kind7374(Kind7374Parsed),
    #[serde(rename = "7375")]
    Kind7375(Kind7375Parsed),
    #[serde(rename = "7376")]
    Kind7376(Kind7376Parsed),
    #[serde(rename = "9321")]
    Kind9321(Kind9321Parsed),
    #[serde(rename = "9735")]
    Kind9735(Kind9735Parsed),
    #[serde(rename = "10002")]
    Kind10002(Kind10002Parsed),
    #[serde(rename = "10019")]
    Kind10019(Kind10019Parsed),
    #[serde(rename = "17375")]
    Kind17375(Kind17375Parsed),
    #[serde(rename = "39089")]
    Kind39089(crate::parser::Kind39089Parsed),
}

impl ParsedData {
    /// Build FlatBuffer for the parsed data, returning the union type and offset
    pub fn build_flatbuffer<'a, A: flatbuffers::Allocator + 'a>(
        &self,
        builder: &mut flatbuffers::FlatBufferBuilder<'a, A>,
    ) -> anyhow::Result<(
        crate::generated::nostr::fb::ParsedData,
        flatbuffers::WIPOffset<flatbuffers::UnionWIPOffset>,
    )> {
        use crate::generated::nostr::fb;

        match self {
            ParsedData::Kind0(data) => {
                let offset = crate::parser::kind0::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind0Parsed, offset.as_union_value()))
            }
            ParsedData::Kind1(data) => {
                let offset = crate::parser::kind1::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind1Parsed, offset.as_union_value()))
            }
            ParsedData::Kind3(data) => {
                let offset = crate::parser::kind3::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind3Parsed, offset.as_union_value()))
            }
            ParsedData::Kind4(data) => {
                let offset = crate::parser::kind4::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind4Parsed, offset.as_union_value()))
            }
            ParsedData::Kind6(data) => {
                let offset = crate::parser::kind6::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind6Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7(data) => {
                let offset = crate::parser::kind7::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7Parsed, offset.as_union_value()))
            }
            ParsedData::Kind17(data) => {
                let offset = crate::parser::kind17::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind17Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7374(data) => {
                let offset = crate::parser::kind7374::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7374Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7375(data) => {
                let offset = crate::parser::kind7375::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7375Parsed, offset.as_union_value()))
            }
            ParsedData::Kind7376(data) => {
                let offset = crate::parser::kind7376::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind7376Parsed, offset.as_union_value()))
            }
            ParsedData::Kind9321(data) => {
                let offset = crate::parser::kind9321::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind9321Parsed, offset.as_union_value()))
            }
            ParsedData::Kind9735(data) => {
                let offset = crate::parser::kind9735::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind9735Parsed, offset.as_union_value()))
            }
            ParsedData::Kind10002(data) => {
                let offset = crate::parser::kind10002::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind10002Parsed, offset.as_union_value()))
            }
            ParsedData::Kind10019(data) => {
                let offset = crate::parser::kind10019::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind10019Parsed, offset.as_union_value()))
            }
            ParsedData::Kind17375(data) => {
                let offset = crate::parser::kind17375::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind17375Parsed, offset.as_union_value()))
            }
            ParsedData::Kind39089(data) => {
                let offset = crate::parser::kind39089::build_flatbuffer(data, builder)?;
                Ok((fb::ParsedData::Kind39089Parsed, offset.as_union_value()))
            }
        }
    }
}

/// ParsedEvent represents a Nostr event with additional parsed data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedEvent {
    #[serde(flatten)]
    pub event: Event,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parsed: Option<ParsedData>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub requests: Option<Vec<Request>>,

    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub relays: Vec<String>,
}

impl ParsedEvent {
    pub fn new(event: Event) -> Self {
        Self {
            event,
            parsed: None,
            requests: None,
            relays: Vec::new(),
        }
    }

    pub fn with_parsed(mut self, parsed: ParsedData) -> Self {
        self.parsed = Some(parsed);
        self
    }

    pub fn with_relays(mut self, relays: Vec<String>) -> Self {
        self.relays = relays;
        self
    }

    pub fn with_requests(mut self, requests: Vec<Request>) -> Self {
        self.requests = Some(requests);
        self
    }
}

/// Signer types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SignerType {
    #[serde(rename = "privkey")]
    PrivKey,
}

impl std::fmt::Display for SignerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SignerType::PrivKey => write!(f, "privkey"),
        }
    }
}

impl std::str::FromStr for SignerType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "privkey" => Ok(SignerType::PrivKey),
            _ => Err(anyhow::anyhow!("Unknown signer type: {}", s)),
        }
    }
}

/// Message types for WebAssembly communication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SignerMessage {
    #[serde(rename = "SIGNED")]
    Signed { payload: Vec<u8> },

    #[serde(rename = "PUBKEY")]
    PubKey { payload: String },

    #[serde(rename = "NIP04_ENCRYPTED")]
    Nip04Encrypted { payload: String },

    #[serde(rename = "NIP04_DECRYPTED")]
    Nip04Decrypted { payload: String },

    #[serde(rename = "NIP44_ENCRYPTED")]
    Nip44Encrypted { payload: String },

    #[serde(rename = "NIP44_DECRYPTED")]
    Nip44Decrypted { payload: String },

    #[serde(rename = "ERROR")]
    Error { message: String },
}

/// Network event types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkEventType {
    Event,
    EOSE,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkEvent {
    pub event_type: NetworkEventType,
    pub event: Option<Event>,
    pub error: Option<String>,
    pub relay: Option<String>,
}

/// Publish status types
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PublishStatus {
    Pending,
    Sent,
    Success,
    Failed,
    Rejected,
    ConnectionError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayStatusUpdate {
    pub relay: String,
    pub status: PublishStatus,
    pub message: String,
    pub timestamp: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishSummary {
    pub relay_count: usize,
    pub success_count: usize,
    pub relay_statuses: Vec<RelayStatusUpdate>,
    pub duration_ms: u64,
    pub timestamp: i64,
}

/// Re-export common types that might be used across modules
pub type EventKind = i32;
pub type RelayUrl = String;
pub type EventJson = String;
pub type PubkeyHex = String;
pub type EventIdHex = String;

/// Common result type for this module
pub type TypesResult<T> = Result<T, TypesError>;

/// Error types for the types module
#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Missing field: {0}")]
    MissingField(String),

    #[error("Invalid version: {0}")]
    InvalidVersion(i32),
}

impl From<TypesError> for JsValue {
    fn from(err: TypesError) -> Self {
        JsValue::from_str(&err.to_string())
    }
}
