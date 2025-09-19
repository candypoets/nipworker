//! Types module for Nutscash Nostr
//!
//! This module contains all the type definitions used throughout the Nostr implementation,
//! including event types, request types, proof types, and communication types.

pub mod network;
pub mod nostr;
pub mod parsed_event;
pub mod proof;

// Re-export module types
pub use proof::{DleqProof, Proof, TokenContent};

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

use wasm_bindgen::prelude::*;

/// Common result type for this module
pub type TypesResult<T> = Result<T, TypesError>;

/// Error types for the types module
#[derive(Debug, thiserror::Error)]
pub enum TypesError {
    #[error("Invalid format: {0}")]
    InvalidFormat(String),

    #[error("Missing field: {0}")]
    MissingField(String),

    #[error("Invalid version: {0}")]
    InvalidVersion(i32),

    #[error("Other error: {0}")]
    Other(String),
}

impl From<crate::parser::ParserError> for TypesError {
    fn from(err: crate::parser::ParserError) -> Self {
        TypesError::Other(err.to_string())
    }
}

impl From<flatbuffers::InvalidFlatbuffer> for TypesError {
    fn from(err: flatbuffers::InvalidFlatbuffer) -> Self {
        TypesError::Other(err.to_string())
    }
}

impl From<TypesError> for JsValue {
    fn from(err: TypesError) -> Self {
        JsValue::from_str(&err.to_string())
    }
}
