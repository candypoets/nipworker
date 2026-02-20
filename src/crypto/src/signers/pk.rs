use tracing::info;
use wasm_bindgen::prelude::*;

use crate::signers::{
    nip44::{decrypt, encrypt, ConversationKey},
    SignerError,
};
use shared::types::{Event, Keys, PublicKey, SecretKey};
use shared::types::nostr::Template;

use signature::hazmat::{PrehashSigner, PrehashVerifier};

type Result<T> = std::result::Result<T, SignerError>;

/// PrivateKeySigner (skeleton)
///
/// This is a browser-friendly skeleton that will later wire into real crypto:
/// - Deriving the public key from a secret (hex or nsec)
/// - Schnorr signing of Nostr events (BIP-340)
/// - NIP-04 / NIP-44 encryption and decryption
///
pub struct PrivateKeySigner {
    /// Secret key representation provided by the caller.
    /// Accepts hex (64 lowercase hex chars) or bech32 nsec.
    keys: Keys,
    /// Cached public key (hex, x-only) derived from `keys`.
    pubkey_hex: String,
}

impl PrivateKeySigner {
    /// Construct a new PrivateKeySigner from a secret string.
    ///
    /// - Accepts hex (64 hex chars) or bech32 nsec starting with "nsec".
    /// - This skeleton does NOT derive or validate the actual key bytes yet.
    pub fn new(private_key_hex: &str) -> Result<Self> {
        let secret_key = SecretKey::from_hex(private_key_hex)
            .map_err(|e| SignerError::Other(format!("Invalid private key: {}", e)))?;
        let keys = Keys::new(secret_key);
        let pubkey_hex = keys.public_key().to_hex();

        info!(
            "Created new PrivateKeySigner with public key: {}",
            pubkey_hex
        );

        Ok(Self { keys, pubkey_hex })
    }

    /// Replace the secret.
    pub fn set_secret(&mut self, secret: &str) -> Result<()> {
        let secret_key = SecretKey::from_hex(secret)
            .map_err(|e| SignerError::Other(format!("Invalid private key: {}", e)))?;
        self.keys = Keys::new(secret_key);
        self.pubkey_hex = self.keys.public_key().to_hex();
        info!("[pk] signer secret replaced");
        Ok(())
    }

    /// Return the public key (hex, x-only).
    pub fn get_public_key(&self) -> Result<String> {
        info!("[pk] get_public_key called: {}", self.pubkey_hex);
        Ok(self.pubkey_hex.clone())
    }

    /// Sign a template into a full signed event
    pub async fn sign_event(&self, template_json: &str) -> Result<String> {
        // Parse the template JSON
        let template = Template::from_json(template_json)
            .map_err(|e| SignerError::Other(format!("Failed to parse template JSON: {}", e)))?;

        // Create an event from the template
        let mut event = Event {
            id: shared::types::EventId([0u8; 32]),
            pubkey: self.keys.public_key(),
            created_at: template.created_at,
            kind: template.kind,
            tags: template.tags,
            content: template.content,
            sig: String::new(),
        };

        // Compute the event ID
        let event_id_hex = shared::nostr_crypto::compute_event_id(
            &event.pubkey,
            event.created_at,
            event.kind,
            &event.tags,
            &event.content,
        );
        event.id = shared::types::EventId::from_hex(&event_id_hex)
            .map_err(|e| SignerError::Other(format!("Failed to parse event ID: {}", e)))?;

        // Sign the event
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Create k256 Schnorr signing key from secret key bytes
        let signing_key = k256::schnorr::SigningKey::from_bytes(&secret_key.0)
            .map_err(|e| SignerError::Other(format!("Failed to create signing key: {}", e)))?;

        let verifying_key = signing_key.verifying_key();

        // Sign the 32-byte event id as a prehash message
        let signature = signing_key
            .sign_prehash(&event.id.to_bytes())
            .map_err(|e| SignerError::Other(format!("Schnorr prehash sign failed: {}", e)))?;

        // Verify with the prehash verifier to match nostr-tools/relay behavior
        verifying_key
            .verify_prehash(&event.id.to_bytes(), &signature)
            .map_err(|e| {
                SignerError::Other(format!("Local Schnorr prehash verify failed: {}", e))
            })?;

        // Set the signature on the event
        event.sig = hex::encode(signature.to_bytes());

        // Return the signed event as JSON
        Ok(event.to_json())
    }

    /// NIP-04 encrypt plaintext for a recipient pubkey (hex).
    pub fn nip04_encrypt(&self, _recipient_pubkey_hex: &str, _plaintext: &str) -> Result<String> {
        let recipient_pk = if _recipient_pubkey_hex.is_empty() {
            self.keys.public_key()
        } else {
            PublicKey::from_hex(_recipient_pubkey_hex)
                .map_err(|e| SignerError::Other(format!("Invalid recipient public key: {}", e)))?
        };
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Use the real NIP-04 encryption
        let encrypted = super::nip04::encrypt(&secret_key, &recipient_pk, _plaintext)
            .map_err(|e| SignerError::CryptoError(format!("NIP-04 encryption failed: {}", e)))?;

        Ok(encrypted)
    }

    /// NIP-04 decrypt ciphertext from a sender pubkey (hex).
    pub fn nip04_decrypt(&self, _sender_pubkey_hex: &str, _ciphertext: &str) -> Result<String> {
        let sender_pk = PublicKey::from_hex(_sender_pubkey_hex)
            .map_err(|e| SignerError::Other(format!("Invalid sender public key: {}", e)))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Use the real NIP-04 decryption
        let decrypted = super::nip04::decrypt(&secret_key, &sender_pk, _ciphertext)
            .map_err(|e| SignerError::CryptoError(format!("NIP-04 decryption failed: {}", e)))?;

        Ok(decrypted)
    }

    /// NIP-44 encrypt plaintext for a recipient pubkey (hex).
    pub fn nip44_encrypt(&self, _recipient_pubkey_hex: &str, _plaintext: &str) -> Result<String> {
        let recipient_pk = if _recipient_pubkey_hex.is_empty() {
            self.keys.public_key()
        } else {
            PublicKey::from_hex(_recipient_pubkey_hex)
                .map_err(|e| SignerError::Other(format!("Invalid recipient public key: {}", e)))?
        };
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Derive conversation key from our secret key and recipient's public key
        let conversation_key = ConversationKey::derive(&secret_key, &recipient_pk)?;

        // Encrypt the plaintext using NIP-44
        let encrypted = encrypt(_plaintext, &conversation_key)?;

        Ok(encrypted)
    }

    /// NIP-44 decrypt ciphertext from a sender pubkey (hex).
    pub fn nip44_decrypt(&self, _sender_pubkey_hex: &str, _ciphertext: &str) -> Result<String> {
        let sender_pk = PublicKey::from_hex(_sender_pubkey_hex)
            .map_err(|e| SignerError::Other(format!("Invalid sender public key: {}", e)))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Derive conversation key from secret key and sender's public key
        let conversation_key = ConversationKey::derive(&secret_key, &sender_pk)?;

        // Decrypt the ciphertext using NIP-44
        let decrypted = decrypt(_ciphertext, &conversation_key)?;

        Ok(decrypted)
    }

    /// NIP-04 decrypt when both participants are provided (sender/recipient).
    /// Chooses the correct peer based on our pubkey and delegates to nip04_decrypt.
    pub fn nip04_decrypt_between(
        &self,
        _sender_pubkey_hex: &str,
        _recipient_pubkey_hex: &str,
        _ciphertext: &str,
    ) -> Result<String> {
        let peer_hex = if self.pubkey_hex == _sender_pubkey_hex {
            _recipient_pubkey_hex
        } else {
            _sender_pubkey_hex
        };
        self.nip04_decrypt(peer_hex, _ciphertext)
    }

    /// NIP-44 decrypt when both participants are provided (sender/recipient).
    /// Chooses the correct peer based on our pubkey and delegates to nip44_decrypt.
    pub fn nip44_decrypt_between(
        &self,
        _sender_pubkey_hex: &str,
        _recipient_pubkey_hex: &str,
        _ciphertext: &str,
    ) -> Result<String> {
        let peer_hex = if self.pubkey_hex == _sender_pubkey_hex {
            _recipient_pubkey_hex
        } else {
            _sender_pubkey_hex
        };
        self.nip44_decrypt(peer_hex, _ciphertext)
    }
}

// ---------------
// Helper utilities
// ---------------

fn js_err(msg: &str) -> JsValue {
    JsValue::from_str(msg)
}

fn is_hex64(s: &str) -> bool {
    if s.len() != 64 {
        return false;
    }
    s.bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f' | b'A'..=b'F'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_hex64() {
        assert!(is_hex64(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_hex64("0123456789abcdef")); // too short
        assert!(!is_hex64(
            "z123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )); // invalid char
    }
}
