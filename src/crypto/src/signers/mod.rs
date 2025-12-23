#![allow(dead_code)]

/*!
Signers module

This module groups the three signer implementations behind clear modules:
- `pk`    : Direct private key signer (local Schnorr signing, NIP-04/44 enc/dec)
- `nip07` : Browser signer via window.nostr (NIP-07 provider)
- `nip46` : Remote signer over relays (Nostr Connect / NIP-46)

Each implementation should expose a minimal, browser-friendly async API without requiring
async_trait, and can be composed by higher-level managers.
*/
pub mod nip04;
pub mod nip07;
pub mod nip44;
pub mod nip46;
pub mod pk;
pub mod types;

// Re-exports for convenience
pub use nip07::Nip07Signer;
pub use nip46::{Nip46Config, Nip46Signer};
pub use pk::PrivateKeySigner;

/// Kinds of signer supported by this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignerKind {
    PrivateKey,
    Nip07,
    Nip46,
}

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
