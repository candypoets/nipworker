use async_trait::async_trait;
use nipworker_core::crypto::nostr_crypto::{compute_event_id, sign_event};
use nipworker_core::traits::{Signer, SignerError};
use nipworker_core::types::nostr::{Event, EventId, Keys, SecretKey, Template};
use std::cell::RefCell;

/// WASM-compatible signer wrapping core cryptographic primitives.
/// NIP-07 and NIP-46 can be added later as additional variants.
pub struct LocalSigner {
    inner: RefCell<Option<Keys>>,
}

impl std::fmt::Debug for LocalSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalSigner")
            .field("configured", &self.inner.borrow().is_some())
            .finish()
    }
}

impl LocalSigner {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(None),
        }
    }

    pub fn set_private_key(&self, secret: &str) -> Result<(), SignerError> {
        let secret_key = SecretKey::from_hex(secret)
            .map_err(|e| SignerError::Other(format!("Invalid private key: {}", e)))?;
        let keys = Keys::new(secret_key);
        *self.inner.borrow_mut() = Some(keys);
        Ok(())
    }
}

#[async_trait(?Send)]
impl Signer for LocalSigner {
    async fn get_public_key(&self) -> Result<String, SignerError> {
        if let Some(keys) = self.inner.borrow().as_ref() {
            Ok(keys.public_key().to_hex())
        } else {
            Err(SignerError::Other("Signer not configured".to_string()))
        }
    }

    async fn sign_event(&self, event_json: &str) -> Result<String, SignerError> {
        let keys = self.inner.borrow();
        let keys = keys.as_ref().ok_or_else(|| SignerError::Other("Signer not configured".to_string()))?;

        let template = Template::from_json(event_json)
            .map_err(|e| SignerError::Other(format!("Failed to parse template JSON: {}", e)))?;

        let mut event = Event {
            id: EventId([0u8; 32]),
            pubkey: keys.public_key(),
            created_at: template.created_at,
            kind: template.kind,
            tags: template.tags,
            content: template.content,
            sig: String::new(),
        };

        let event_id_hex = compute_event_id(
            &event.pubkey,
            event.created_at,
            event.kind,
            &event.tags,
            &event.content,
        );
        event.id = EventId::from_hex(&event_id_hex)
            .map_err(|e| SignerError::Other(format!("Failed to parse event ID: {}", e)))?;

        let secret_key = keys.secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;
        event.sig = sign_event(secret_key, &event.id)
            .map_err(|e| SignerError::Other(format!("Sign event failed: {}", e)))?;

        Ok(event.to_json())
    }

    async fn nip04_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
        Err(SignerError::Other("NIP-04 encrypt not implemented in engine signer".to_string()))
    }

    async fn nip04_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
        Err(SignerError::Other("NIP-04 decrypt not implemented in engine signer".to_string()))
    }

    async fn nip44_encrypt(&self, _peer: &str, _plaintext: &str) -> Result<String, SignerError> {
        Err(SignerError::Other("NIP-44 encrypt not implemented in engine signer".to_string()))
    }

    async fn nip44_decrypt(&self, _peer: &str, _ciphertext: &str) -> Result<String, SignerError> {
        Err(SignerError::Other("NIP-44 decrypt not implemented in engine signer".to_string()))
    }
}
