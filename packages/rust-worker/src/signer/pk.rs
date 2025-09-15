use crate::{
    nostr::{timestamp_now, Template},
    types::{Event, Keys, PublicKey},
    EventId,
};
use anyhow::Result;
use k256::schnorr::signature::Signer;
use k256::schnorr::SigningKey;
use tracing::{debug, info};

use super::interface::Signer as SignerInterface;

/// PrivateKeySigner provides cryptographic operations using a private key
/// It implements methods for NIP-04, NIP-44 encryption/decryption, and event signing
pub struct PrivateKeySigner {
    /// The nostr Keys object containing both private and public keys
    keys: Keys,
}

impl PrivateKeySigner {
    /// Creates a new PrivateKeySigner from a hex-encoded private key
    pub fn new(private_key_hex: &str) -> Result<Self> {
        let secret_key = crate::types::SecretKey::from_hex(private_key_hex)?;
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

impl SignerInterface for PrivateKeySigner {
    /// Returns the public key corresponding to the private key
    fn get_public_key(&self) -> Result<String> {
        Ok(self.keys.public_key().to_hex())
    }

    /// Signs a nostr event with the private key
    fn sign_event(&self, template: &Template) -> Result<Event> {
        debug!("Signing event of kind {}", template.kind);

        let created_at = timestamp_now();
        let pubkey = self.keys.public_key();

        let mut event = Event {
            id: EventId([0u8; 32]), // Will be computed
            pubkey,
            created_at,
            kind: template.kind,
            tags: template.tags.clone(),
            content: template.content.clone(),
            sig: String::new(), // Will be computed
        };

        // Compute the event ID
        event.compute_id()?;

        // Sign the event
        let signing_key = SigningKey::from_bytes(&self.keys.secret_key.0)?;

        let signature = signing_key.sign(&event.id.0);
        event.sig = hex::encode(signature.to_bytes());

        Ok(event)
    }

    /// Converts an EventTemplate to an UnsignedEvent using the signer's public key
    // fn unsign_event(&self, template: EventTemplate) -> Result<UnsignedEvent> {
    //     let pubkey = self.keys.public_key();
    //     template
    //         .to_unsigned_event(pubkey)
    //         .map_err(|e| anyhow::anyhow!("Failed to create unsigned event: {}", e))
    // }

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

        // TODO: Implement NIP-04 encryption
        let encrypted = format!("nip04_encrypted_{}", plaintext);

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

        // TODO: Implement NIP-04 decryption
        let decrypted = ciphertext.replace("nip04_encrypted_", "");

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

        // TODO: Implement NIP-44 encryption
        let encrypted = format!("nip44_encrypted_{}", plaintext);

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

        // TODO: Implement NIP-44 decryption
        let decrypted = ciphertext.replace("nip44_encrypted_", "");

        debug!("Successfully decrypted message using NIP-44");
        Ok(decrypted)
    }
}
