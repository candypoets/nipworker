//! Types module for Nutscash Nostr
//!
//! This module contains all the type definitions used throughout the Nostr implementation,
//! including event types, request types, proof types, and communication types.

pub mod network;
pub mod parsed_event;
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
