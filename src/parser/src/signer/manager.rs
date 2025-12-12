use shared::generated::nostr::fb;
use crate::signer::SignerError;
use crate::types::nostr::{Event, Template};

use std::sync::{Arc, Mutex};

type Result<T> = std::result::Result<T, SignerError>;
use tracing::{debug, info};

use super::interface::{SignerInterface, SignerManagerInterface};
use super::pk::PrivateKeySigner;

/// SignerManagerImpl handles event signing operations and manages the current signer
pub struct SignerManager {
    current: Arc<Mutex<Option<Box<dyn SignerInterface>>>>,
    signer_type: Arc<Mutex<Option<fb::SignerType>>>,
}

impl SignerManager {
    /// Creates a new signer manager
    pub fn new() -> Self {
        info!("Creating new SignerManager");

        Self {
            current: Arc::new(Mutex::new(None)),
            signer_type: Arc::new(Mutex::new(None)),
        }
    }
}

impl SignerManagerInterface for SignerManager {
    /// Signs an event with the current signer
    fn sign_event(&self, event: &Template) -> Result<Event> {
        let current_lock = self.current.lock().unwrap();
        let signer = current_lock.as_ref().ok_or_else(|| SignerError::NoSigner)?;

        let signed_event = signer.sign_event(event)?;

        Ok(signed_event)
    }

    // /// Converts an EventTemplate to an UnsignedEvent using the current signer's public key
    // fn unsign_event(&self, template: &Template) -> Result<UnsignedEvent> {
    //     info!(
    //         "Converting EventTemplate to UnsignedEvent for kind {}",
    //         template.kind
    //     );

    //     let current_lock = self.current.lock().unwrap();
    //     let signer = current_lock
    //         .as_ref()
    //         .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

    //     let unsigned_event = signer.unsign_event(template)?;

    //     debug!("Successfully created UnsignedEvent");
    //     Ok(unsigned_event)
    // }

    /// Returns the public key of the current signer
    fn get_public_key(&self) -> Result<String> {
        let current_lock = self.current.lock().unwrap();
        let signer = current_lock.as_ref().ok_or_else(|| SignerError::NoSigner)?;

        let pubkey = signer.get_public_key()?;

        Ok(pubkey)
    }

    fn set_privatekey_signer(&self, private_key_hex: &str) -> Result<()> {
        self.current
            .lock()
            .unwrap()
            .replace(Box::new(PrivateKeySigner::new(private_key_hex)?));

        return Ok(());
    }

    /// Gets the current signer type
    fn get_signer_type(&self) -> Option<fb::SignerType> {
        let type_lock = self.signer_type.lock().unwrap();
        type_lock.clone()
    }

    /// Returns whether a signer is currently set
    fn has_signer(&self) -> bool {
        let current_lock = self.current.lock().unwrap();
        current_lock.is_some()
    }

    /// Encrypts a message for a recipient using NIP-04
    fn nip04_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String> {
        let current_lock = self.current.lock().unwrap();
        let signer = current_lock.as_ref().ok_or_else(|| SignerError::NoSigner)?;

        let encrypted = signer.nip04_encrypt(recipient_pubkey, plaintext)?;

        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-04
    fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        let current_lock = self.current.lock().unwrap();
        let signer = current_lock.as_ref().ok_or_else(|| SignerError::NoSigner)?;

        let decrypted = signer.nip04_decrypt(sender_pubkey, ciphertext)?;

        Ok(decrypted)
    }

    /// Encrypts a message for a recipient using NIP-44
    fn nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String> {
        let current_lock = self.current.lock().unwrap();
        let signer = current_lock.as_ref().ok_or_else(|| SignerError::NoSigner)?;

        let encrypted = signer.nip44_encrypt(recipient_pubkey, plaintext)?;

        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-44
    fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        let current_lock = self.current.lock().unwrap();
        let signer = current_lock.as_ref().ok_or_else(|| SignerError::NoSigner)?;

        let decrypted = signer.nip44_decrypt(sender_pubkey, ciphertext)?;

        Ok(decrypted)
    }
}

impl Default for SignerManager {
    fn default() -> Self {
        Self::new()
    }
}
