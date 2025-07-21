use anyhow::Result;
use nostr::nips::{nip04, nip44};
use nostr::{Event, EventBuilder, Keys, Kind, PublicKey, Tag, UnsignedEvent};
use tracing::{debug, info};

use super::interface::Signer;
use crate::types::EventTemplate;

/// PrivateKeySigner provides cryptographic operations using a private key
/// It implements methods for NIP-04, NIP-44 encryption/decryption, and event signing
pub struct PrivateKeySigner {
    /// The nostr Keys object containing both private and public keys
    keys: Keys,
}

impl PrivateKeySigner {
    /// Creates a new PrivateKeySigner from a hex-encoded private key
    pub fn new(private_key_hex: &str) -> Result<Self> {
        let secret_key = nostr::SecretKey::from_hex(private_key_hex)?;
        let keys = Keys::new(secret_key);

        info!(
            "Created new PrivateKeySigner with public key: {}",
            keys.public_key().to_hex()
        );

        Ok(Self { keys })
    }

    /// Creates a new PrivateKeySigner from an nsec (bech32-encoded private key)
    pub fn from_nsec(nsec: &str) -> Result<Self> {
        let keys = Keys::parse(nsec)?;

        info!(
            "Created new PrivateKeySigner from nsec with public key: {}",
            keys.public_key().to_hex()
        );

        Ok(Self { keys })
    }

    /// Generates a new random PrivateKeySigner
    pub fn generate() -> Self {
        let keys = Keys::generate();

        info!(
            "Generated new PrivateKeySigner with public key: {}",
            keys.public_key().to_hex()
        );

        Self { keys }
    }

    /// Returns the private key (hex encoded)
    /// This method is not part of the Signer trait for security reasons
    /// but is useful for certain internal operations
    pub fn get_private_key(&self) -> String {
        self.keys.secret_key().unwrap().display_secret().to_string()
    }

    /// Returns the Keys object
    pub fn get_keys(&self) -> &Keys {
        &self.keys
    }
}

impl Signer for PrivateKeySigner {
    /// Returns the public key corresponding to the private key
    fn get_public_key(&self) -> Result<String> {
        Ok(self.keys.public_key().to_hex())
    }

    /// Signs a nostr event with the private key
    fn sign_event(&self, event: &mut UnsignedEvent) -> Result<Event> {
        debug!(
            "Signing event of kind {} with public key {}",
            event.kind,
            self.keys.public_key()
        );

        // Create a new signed event using EventBuilder
        let event_builder = EventBuilder::new(event.kind, &event.content, event.tags.clone());
        let signed_event = event_builder.to_event(&self.keys)?;

        debug!("Successfully signed event: {}", signed_event.id);
        Ok(signed_event)
    }

    /// Converts an EventTemplate to an UnsignedEvent using the signer's public key
    fn unsign_event(&self, template: EventTemplate) -> Result<UnsignedEvent> {
        let pubkey = self.keys.public_key();
        template
            .to_unsigned_event(pubkey)
            .map_err(|e| anyhow::anyhow!("Failed to create unsigned event: {}", e))
    }

    /// Encrypts a message for a recipient using NIP-04
    fn nip04_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String> {
        let recipient_pk = PublicKey::from_hex(recipient_pubkey)
            .map_err(|e| anyhow::anyhow!("Invalid recipient public key: {}", e))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| anyhow::anyhow!("Failed to get secret key: {}", e))?;

        debug!(
            "Encrypting message using NIP-04 for recipient: {}",
            recipient_pubkey
        );

        let encrypted = nip04::encrypt(&secret_key, &recipient_pk, plaintext)
            .map_err(|e| anyhow::anyhow!("NIP-04 encryption failed: {}", e))?;

        debug!("Successfully encrypted message using NIP-04");
        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-04
    fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        let sender_pk = PublicKey::from_hex(sender_pubkey)
            .map_err(|e| anyhow::anyhow!("Invalid sender public key: {}", e))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| anyhow::anyhow!("Failed to get secret key: {}", e))?;

        debug!(
            "Decrypting message using NIP-04 from sender: {}",
            sender_pubkey
        );

        let decrypted = nip04::decrypt(&secret_key, &sender_pk, ciphertext)
            .map_err(|e| anyhow::anyhow!("NIP-04 decryption failed: {}", e))?;

        debug!("Successfully decrypted message using NIP-04");
        Ok(decrypted)
    }

    /// Encrypts a message for a recipient using NIP-44
    fn nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String> {
        let recipient_pk = PublicKey::from_hex(recipient_pubkey)
            .map_err(|e| anyhow::anyhow!("Invalid recipient public key: {}", e))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| anyhow::anyhow!("Failed to get secret key: {}", e))?;

        debug!(
            "Encrypting message using NIP-44 for recipient: {}",
            recipient_pubkey
        );

        let encrypted = nip44::encrypt(&secret_key, &recipient_pk, plaintext, nip44::Version::V2)
            .map_err(|e| anyhow::anyhow!("NIP-44 encryption failed: {}", e))?;

        debug!("Successfully encrypted message using NIP-44");
        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-44
    fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        let sender_pk = PublicKey::from_hex(sender_pubkey)
            .map_err(|e| anyhow::anyhow!("Invalid sender public key: {}", e))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| anyhow::anyhow!("Failed to get secret key: {}", e))?;

        debug!(
            "Decrypting message using NIP-44 from sender: {}",
            sender_pubkey
        );

        let decrypted = nip44::decrypt(&secret_key, &sender_pk, ciphertext)
            .map_err(|e| anyhow::anyhow!("NIP-44 decryption failed: {}", e))?;

        debug!("Successfully decrypted message using NIP-44");
        Ok(decrypted)
    }

    /// Creates a new nostr event, populates it with the given data, and signs it
    fn create_and_sign_event(
        &self,
        kind: i32,
        content: &str,
        tags: Vec<Vec<String>>,
    ) -> Result<Event> {
        debug!(
            "Creating and signing event of kind {} with {} tags",
            kind,
            tags.len()
        );

        // Convert string tags to nostr::Tags
        let nostr_tags: Vec<Tag> = tags
            .into_iter()
            .filter_map(|tag| {
                if tag.is_empty() {
                    None
                } else {
                    // Create a tag using the first element as kind and rest as values
                    if let Some(first) = tag.get(0) {
                        let values = if tag.len() > 1 {
                            tag[1..].to_vec()
                        } else {
                            vec![]
                        };
                        Tag::parse(vec![vec![first.clone()], values].concat()).ok()
                    } else {
                        None
                    }
                }
            })
            .collect();

        let event_builder = EventBuilder::new(Kind::from(kind as u64), content, nostr_tags);
        let event = event_builder.to_event(&self.keys)?;

        debug!("Successfully created and signed event: {}", event.id);
        Ok(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::SecretKey;

    #[test]
    fn test_private_key_signer_creation() {
        let keys = Keys::generate();
        let private_key_hex = format!("{}", keys.secret_key().unwrap().display_secret());

        let signer = PrivateKeySigner::new(&private_key_hex).unwrap();
        let public_key = signer.get_public_key().unwrap();

        assert!(!public_key.is_empty());
        assert_eq!(public_key.len(), 64); // hex public key length
    }

    #[test]
    fn test_event_signing() {
        let signer = PrivateKeySigner::generate();
        let event_builder = EventBuilder::new(Kind::TextNote, "Hello, Nostr!", vec![]);
        let event = event_builder.to_event(&signer.keys).unwrap();

        // Event is already signed, so just verify it works
        assert!(event.verify().is_ok());
        assert_eq!(event.pubkey, signer.keys.public_key());
    }

    #[test]
    fn test_create_and_sign_event() {
        let signer = PrivateKeySigner::generate();
        let tags = vec![
            vec!["p".to_string(), "test_pubkey".to_string()],
            vec!["e".to_string(), "test_event_id".to_string()],
        ];

        let event = signer
            .create_and_sign_event(1, "Test content", tags)
            .unwrap();

        assert_eq!(event.kind, Kind::TextNote);
        assert_eq!(event.content, "Test content");
        assert_eq!(event.tags.len(), 2);
        assert!(event.verify().is_ok());
    }

    #[test]
    fn test_nip04_encryption_decryption() {
        let signer1 = PrivateKeySigner::generate();
        let signer2 = PrivateKeySigner::generate();

        let message = "Secret message";
        let recipient_pubkey = signer2.get_public_key().unwrap();
        let sender_pubkey = signer1.get_public_key().unwrap();

        // Encrypt with signer1's private key and signer2's public key
        let encrypted = signer1.nip04_encrypt(&recipient_pubkey, message).unwrap();

        // Verify the encrypted message is not the same as the original
        assert_ne!(message, encrypted);
        assert!(!encrypted.is_empty());

        // Decrypt with signer2's private key and signer1's public key
        let decrypted = signer2.nip04_decrypt(&sender_pubkey, &encrypted).unwrap();

        // Verify decryption works correctly
        assert_eq!(message, decrypted);
    }

    #[test]
    fn test_nip44_encryption_decryption() {
        let signer1 = PrivateKeySigner::generate();
        let signer2 = PrivateKeySigner::generate();

        let message = "Secret message with NIP-44";
        let recipient_pubkey = signer2.get_public_key().unwrap();
        let sender_pubkey = signer1.get_public_key().unwrap();

        // Encrypt with signer1's private key and signer2's public key
        let encrypted = signer1.nip44_encrypt(&recipient_pubkey, message).unwrap();

        // Verify the encrypted message is not the same as the original
        assert_ne!(message, encrypted);
        assert!(!encrypted.is_empty());

        // Decrypt with signer2's private key and signer1's public key
        let decrypted = signer2.nip44_decrypt(&sender_pubkey, &encrypted).unwrap();

        // Verify decryption works correctly
        assert_eq!(message, decrypted);
    }

    #[test]
    fn test_nip04_invalid_public_key() {
        let signer = PrivateKeySigner::generate();
        let message = "Test message";
        let invalid_pubkey = "invalid_pubkey";

        let result = signer.nip04_encrypt(invalid_pubkey, message);
        assert!(result.is_err());
    }

    #[test]
    fn test_nip44_invalid_public_key() {
        let signer = PrivateKeySigner::generate();
        let message = "Test message";
        let invalid_pubkey = "invalid_pubkey";

        let result = signer.nip44_encrypt(invalid_pubkey, message);
        assert!(result.is_err());
    }

    #[test]
    fn test_nip04_invalid_ciphertext() {
        let signer = PrivateKeySigner::generate();
        let pubkey = signer.get_public_key().unwrap();
        let invalid_ciphertext = "invalid_ciphertext";

        let result = signer.nip04_decrypt(&pubkey, invalid_ciphertext);
        assert!(result.is_err());
    }

    #[test]
    fn test_nip44_invalid_ciphertext() {
        let signer = PrivateKeySigner::generate();
        let pubkey = signer.get_public_key().unwrap();
        let invalid_ciphertext = "invalid_ciphertext";

        let result = signer.nip44_decrypt(&pubkey, invalid_ciphertext);
        assert!(result.is_err());
    }
}
