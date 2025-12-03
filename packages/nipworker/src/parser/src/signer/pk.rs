use super::interface::SignerInterface;
use crate::{
    nostr::{timestamp_now, Template},
    signer::{
        nip44::{decrypt, encrypt, ConversationKey},
        SignerError,
    },
    types::{Event, Keys, PublicKey},
    EventId, SecretKey,
};
use k256::schnorr::signature::{
    RandomizedSigner, // optional randomized signing
    Signer,
    Verifier, // standard
};
use signature::hazmat::{PrehashSigner, PrehashVerifier};

use k256::schnorr::SigningKey;

use tracing::{debug, info};

type Result<T> = std::result::Result<T, SignerError>;

/// PrivateKeySigner provides cryptographic operations using a private key
/// It implements methods for NIP-04, NIP-44 encryption/decryption, and event signing
pub struct PrivateKeySigner {
    /// The nostr Keys object containing both private and public keys
    keys: Keys,
}

impl PrivateKeySigner {
    /// Creates a new PrivateKeySigner from a hex-encoded private key
    pub fn new(private_key_hex: &str) -> Result<Self> {
        let secret_key = SecretKey::from_hex(private_key_hex)
            .map_err(|e| SignerError::Other(format!("Invalid private key: {}", e)))?;
        let keys = Keys::new(secret_key);

        info!(
            "Created new PrivateKeySigner with public key: {}",
            keys.public_key().to_hex()
        );

        Ok(Self { keys })
    }

    /// Creates a new PrivateKeySigner from an nsec (bech32-encoded private key)
    pub fn from_nsec(nsec: &str) -> Result<Self> {
        let keys =
            Keys::parse(nsec).map_err(|e| SignerError::Other(format!("Invalid nsec: {}", e)))?;

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
        event
            .compute_id()
            .map_err(|e| SignerError::Other(format!("Failed to compute event ID: {}", e)))?;

        // Sign the event
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Create k256 Schnorr signing key from secret key bytes
        let signing_key = k256::schnorr::SigningKey::from_bytes(&secret_key.0)
            .map_err(|e| SignerError::Other(format!("Failed to create signing key: {}", e)))?;

        // Derive x-only pubkey (BIP-340) and ensure it matches event.pubkey
        let verifying_key = signing_key.verifying_key();
        let expected_pubkey: [u8; 32] = verifying_key.to_bytes().into();
        if event.pubkey.0 != expected_pubkey {
            return Err(SignerError::Other(
                "Event pubkey does not match the signing key (x-only)".into(),
            ));
        }

        // Sign the 32-byte event id as a prehash message
        let signature = signing_key
            .sign_prehash(&event.id.0)
            .map_err(|e| SignerError::Other(format!("Schnorr prehash sign failed: {}", e)))?;

        // Verify with the prehash verifier to match nostr-tools/relay behavior
        verifying_key
            .verify_prehash(&event.id.0, &signature)
            .map_err(|e| {
                SignerError::Other(format!("Local Schnorr prehash verify failed: {}", e))
            })?;

        // Save signature as lowercase hex
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
            .map_err(|e| SignerError::Other(format!("Invalid recipient public key: {}", e)))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Use the real NIP-04 encryption
        let encrypted = super::nip04::encrypt(&secret_key, &recipient_pk, plaintext)
            .map_err(|e| SignerError::CryptoError(format!("NIP-04 encryption failed: {}", e)))?;

        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-04
    fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        let sender_pk = PublicKey::from_hex(sender_pubkey)
            .map_err(|e| SignerError::Other(format!("Invalid sender public key: {}", e)))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Use the real NIP-04 decryption
        let decrypted = super::nip04::decrypt(&secret_key, &sender_pk, ciphertext)
            .map_err(|e| SignerError::CryptoError(format!("NIP-04 decryption failed: {}", e)))?;

        Ok(decrypted)
    }

    /// Encrypts a message for a recipient using NIP-44
    fn nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String> {
        let recipient_pk = PublicKey::from_hex(recipient_pubkey)
            .map_err(|e| SignerError::Other(format!("Invalid recipient public key: {}", e)))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Derive conversation key from our secret key and recipient's public key
        let conversation_key = ConversationKey::derive(&secret_key, &recipient_pk)?;

        // Encrypt the plaintext using NIP-44
        let encrypted = encrypt(plaintext, &conversation_key)?;

        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-44
    fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        let sender_pk = PublicKey::from_hex(sender_pubkey)
            .map_err(|e| SignerError::Other(format!("Invalid sender public key: {}", e)))?;
        let secret_key = self
            .keys
            .secret_key()
            .map_err(|e| SignerError::Other(format!("Failed to get secret key: {}", e)))?;

        // Derive conversation key from secret key and sender's public key
        let conversation_key = ConversationKey::derive(&secret_key, &sender_pk)?;

        // Decrypt the ciphertext using NIP-44
        let decrypted = decrypt(ciphertext, &conversation_key)?;

        Ok(decrypted)
    }
}
