//! Signer module for Nutscash Nostr
//!
//! This module provides cryptographic signing functionality for Nostr events,
//! including support for different signer types, NIP-04 and NIP-44 encryption/decryption,
//! and WebAssembly integration for browser environments.

pub mod interface;
pub mod manager;
pub mod pk;

// Re-export main types and traits
pub use interface::{Signer, SignerFactory, SignerManager};
pub use manager::{SignerManagerImpl, WasmSignerManager};
pub use pk::PrivateKeySigner;

// Re-export types from the types module
pub use crate::types::{SignerMessage, SignerType};

use anyhow::Result;
use std::sync::Arc;

/// Default signer factory implementation
pub struct DefaultSignerFactory;

impl interface::SignerFactory for DefaultSignerFactory {
    fn create_signer(signer_type: SignerType, data: &str) -> Result<Box<dyn Signer>> {
        match signer_type {
            SignerType::PrivKey => {
                if data.is_empty() {
                    // Generate new private key if no data provided
                    Ok(Box::new(PrivateKeySigner::generate()))
                } else if data.starts_with("nsec") {
                    // Handle nsec format (bech32 encoded)
                    Ok(Box::new(PrivateKeySigner::from_nsec(data)?))
                } else {
                    // Handle hex format
                    Ok(Box::new(PrivateKeySigner::new(data)?))
                }
            }
        }
    }
}

/// Convenience function to create a new signer
pub fn create_signer(signer_type: SignerType, data: &str) -> Result<Box<dyn Signer>> {
    DefaultSignerFactory::create_signer(signer_type, data)
}

/// Convenience function to create a new signer manager
pub fn create_signer_manager() -> SignerManagerImpl {
    SignerManagerImpl::new()
}

/// Convenience function to create a private key signer with a generated key
pub fn create_generated_signer() -> PrivateKeySigner {
    PrivateKeySigner::generate()
}

/// Shared signer manager instance for use across the application
pub type SharedSignerManager = Arc<dyn SignerManager + Send + Sync>;

/// Create a shared signer manager instance
pub fn create_shared_signer_manager() -> SharedSignerManager {
    Arc::new(SignerManagerImpl::new())
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

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("MessagePack error: {0}")]
    MessagePackError(#[from] rmp_serde::encode::Error),

    #[error("Nostr error: {0}")]
    NostrError(String),
}

// Error conversions removed - using our minimal nostr types now

impl From<anyhow::Error> for SignerError {
    fn from(err: anyhow::Error) -> Self {
        SignerError::CryptoError(err.to_string())
    }
}

/// Result type for signer operations
pub type SignerResult<T> = Result<T, SignerError>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SignerType;

    #[test]
    fn test_create_signer() {
        let signer = create_signer(SignerType::PrivKey, "").unwrap();
        assert!(signer.get_public_key().is_ok());
    }

    #[test]
    fn test_create_generated_signer() {
        let signer = create_generated_signer();
        assert!(signer.get_public_key().is_ok());
    }

    #[test]
    fn test_create_signer_manager() {
        let manager = create_signer_manager();
        assert!(!manager.has_signer());
    }

    #[test]
    fn test_create_shared_signer_manager() {
        let shared_manager = create_shared_signer_manager();
        assert!(!shared_manager.has_signer());
    }

    #[test]
    fn test_default_signer_factory() {
        let signer = DefaultSignerFactory::create_signer(SignerType::PrivKey, "").unwrap();
        assert!(signer.get_public_key().is_ok());
    }
}
