use crate::generated::nostr::fb;
use crate::signer::SignerError;
use crate::types::nostr::{Event, Template};

type Result<T> = std::result::Result<T, SignerError>;

/// Signer defines the trait for cryptographic operations in Nostr
/// This trait allows for different implementations (e.g., in-memory keys, hardware keys, etc.)
pub trait SignerInterface {
    /// Returns the public key for this signer
    fn get_public_key(&self) -> Result<String>;

    /// Signs a Nostr event with the private key
    fn sign_event(&self, event: &Template) -> Result<Event>;

    /// Encrypts a message for a recipient using NIP-04
    fn nip04_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String>;

    /// Decrypts a message from a sender using NIP-04
    fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String>;

    /// Encrypts a message for a recipient using NIP-44
    fn nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String>;

    /// Decrypts a message from a sender using NIP-44
    fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String>;
}

/// SignerManager handles event signing operations
pub trait SignerManagerInterface {
    /// Signs an event with the current signer
    fn sign_event(&self, event: &Template) -> Result<Event>;

    /// Returns the public key of the current signer
    fn get_public_key(&self) -> Result<String>;

    fn set_privatekey_signer(&self, private_key_hex: &str) -> Result<()>;

    /// Gets the current signer type
    fn get_signer_type(&self) -> Option<fb::SignerType>;

    /// Returns whether a signer is currently set
    fn has_signer(&self) -> bool;

    /// Encrypts a message for a recipient using NIP-04
    fn nip04_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String>;

    /// Decrypts a message from a sender using NIP-04
    fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String>;

    /// Encrypts a message for a recipient using NIP-44
    fn nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String>;

    /// Decrypts a message from a sender using NIP-44
    fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String>;
}
