use nipworker_core::traits::{Signer, SignerError};
use std::sync::Mutex;

pub struct NativeSigner {
    inner: Mutex<Option<nipworker_core::crypto::signers::PrivateKeySigner>>,
}

impl NativeSigner {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(None),
        }
    }

    pub fn set_private_key(&self, secret: &str) -> Result<(), SignerError> {
        let signer = nipworker_core::crypto::signers::PrivateKeySigner::new(secret)
            .map_err(|e| SignerError::Other(format!("Failed to create signer: {}", e)))?;
        *self.inner.lock().unwrap() = Some(signer);
        Ok(())
    }

    fn with_signer<T>(&self, f: impl FnOnce(&nipworker_core::crypto::signers::PrivateKeySigner) -> Result<T, nipworker_core::crypto::signers::SignerError>) -> Result<T, SignerError> {
        let signer = {
            let inner = self.inner.lock().unwrap();
            inner.clone()
        };
        if let Some(signer) = signer.as_ref() {
            f(signer).map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }
}

#[async_trait::async_trait(?Send)]
impl Signer for NativeSigner {
    async fn get_public_key(&self) -> Result<String, SignerError> {
        self.with_signer(|s| s.get_public_key())
    }

    async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
        let signer = {
            let inner = self.inner.lock().unwrap();
            inner.clone()
        };
        if let Some(signer) = signer {
            signer.sign_event(event_json).await
                .map_err(|e| SignerError::Other(e.to_string()))
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn nip04_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
        self.with_signer(|s| s.nip04_encrypt(peer, plaintext))
    }

    async fn nip04_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
        self.with_signer(|s| s.nip04_decrypt(peer, ciphertext))
    }

    async fn nip44_encrypt(&self, peer: &str, plaintext: &str) -> Result<String, SignerError> {
        self.with_signer(|s| s.nip44_encrypt(peer, plaintext))
    }

    async fn nip44_decrypt(&self, peer: &str, ciphertext: &str) -> Result<String, SignerError> {
        self.with_signer(|s| s.nip44_decrypt(peer, ciphertext))
    }
}
