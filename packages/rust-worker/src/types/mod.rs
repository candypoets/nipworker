//! Types module for Nutscash Nostr
//!
//! This module contains all the type definitions used throughout the Nostr implementation,
//! including event types, request types, proof types, and communication types.

pub mod network;
pub mod nostr;
pub mod parsed_event;
pub mod proof;
pub mod thread;

// Re-export module types
pub use network::EOSE;
pub use proof::Proof;
pub use thread::*;

// Re-export nostr types for convenience
pub use crate::types::nostr::{
    Event, EventId, Filter, Keys, PublicKey, SecretKey, Timestamp, UnsignedEvent, SECP256K1,
};

// Re-export Kind helpers
pub use crate::types::nostr::{
    CONTACT_LIST, DELETION, ENCRYPTED_DIRECT_MESSAGE, METADATA, REACTION, RELAY_LIST, REPOST,
    TEXT_NOTE,
};

// Type alias for Kind
pub type Kind = u64;
use serde::{Deserialize, Serialize};

use wasm_bindgen::prelude::*;

use crate::types::network::Request;

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
