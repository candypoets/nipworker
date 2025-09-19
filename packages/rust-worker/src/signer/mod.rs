//! Signer module for Nutscash Nostr
//!
//! This module provides cryptographic signing functionality for Nostr events,
//! including support for different signer types, NIP-04 and NIP-44 encryption/decryption,
//! and WebAssembly integration for browser environments.

pub mod interface;
pub mod manager;
pub mod nip04;
pub mod nip44;
pub mod pk;

// Re-export main types and traits
pub use interface::{SignerInterface, SignerManagerInterface};
pub use manager::SignerManager;
pub use pk::PrivateKeySigner;

/// Error types specific to the signer module
#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error("No signer available")]
    NoSigner,

    #[error("Invalid signer type: {0}")]
    InvalidSignerType(String),

    #[error("Invalid private key format: {0}")]
    InvalidPrivateKey(String),

    #[error("Cryptographic operation failed: {0}")]
    CryptoError(String),

    #[error("Hex decode error: {0}")]
    HexDecodeError(#[from] hex::FromHexError),

    #[error("Nostr error: {0}")]
    NostrError(String),

    #[error("Other error: {0}")]
    Other(String),
}

// Error conversions removed - using our minimal nostr types now

/// Result type for signer operations
pub type SignerResult<T> = Result<T, SignerError>;
