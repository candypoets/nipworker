use async_trait::async_trait;
use nipworker_core::crypto::signers::PrivateKeySigner;
use nipworker_core::traits::{Signer, SignerError};
use std::cell::RefCell;

/// WASM-compatible signer wrapping the core PrivateKeySigner.
/// NIP-07 and NIP-46 can be added later as additional variants.
#[derive(Debug)]
pub struct LocalSigner {
    inner: RefCell<Option<PrivateKeySigner>>,
}

impl LocalSigner {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(None),
        }
    }

    pub fn set_private_key(&self, secret: &str) -> Result<(), SignerError> {
        let signer = PrivateKeySigner::new(secret)
            .map_err(|e| SignerError::Other(e.to_string()))?;
        *self.inner.borrow_mut() = Some(signer);
        Ok(())
    }
}

#[async_trait(?Send)]
impl Signer for LocalSigner {
    async fn get_public_key(&self) -> Result<String, SignerError> {
        if let Some(signer) = self.inner.borrow().as_ref() {
            signer.get_public_key().map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
        if let Some(signer) = self.inner.borrow().as_ref() {
            signer.sign_event(event_json).await.map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
        if let Some(signer) = self.inner.borrow().as_ref() {
            signer.nip04_encrypt(peer, plaintext).map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
        if let Some(signer) = self.inner.borrow().as_ref() {
            signer.nip04_decrypt(peer, ciphertext).map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
        if let Some(signer) = self.inner.borrow().as_ref() {
            signer.nip44_encrypt(peer, plaintext).map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
        if let Some(signer) = self.inner.borrow().as_ref() {
            signer.nip44_decrypt(peer, ciphertext).map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }
}
