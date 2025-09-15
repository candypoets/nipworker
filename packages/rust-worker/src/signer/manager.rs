use crate::types::nostr::{Event, Template, UnsignedEvent};
use crate::types::SignerType;
use anyhow::Result;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, warn};
use wasm_bindgen::prelude::*;

use super::interface::{Signer, SignerManager};
use super::pk::PrivateKeySigner;

/// SignerManagerImpl handles event signing operations and manages the current signer
pub struct SignerManagerImpl {
    current: Arc<Mutex<Option<Box<dyn Signer>>>>,
    signer_type: Arc<Mutex<Option<SignerType>>>,
    callback: Option<js_sys::Function>,
}

impl SignerManagerImpl {
    /// Creates a new signer manager
    pub fn new() -> Self {
        info!("Creating new SignerManager");

        Self {
            current: Arc::new(Mutex::new(None)),
            signer_type: Arc::new(Mutex::new(None)),
            callback: None,
        }
    }

    /// Creates a signer based on the type and data
    fn create_signer(signer_type: SignerType, data: &str) -> Result<Box<dyn Signer>> {
        match signer_type {
            SignerType::PrivKey => {
                if data.is_empty() {
                    // Generate new private key if no data provided
                    Ok(Box::new(PrivateKeySigner::generate()))
                } else if data.starts_with("nsec") {
                    // Handle nsec format
                    Ok(Box::new(PrivateKeySigner::from_nsec(data)?))
                } else {
                    // Handle hex format
                    Ok(Box::new(PrivateKeySigner::new(data)?))
                }
            }
        }
    }
}

impl SignerManager for SignerManagerImpl {
    /// Signs an event with the current signer
    fn sign_event(&self, event: &Template) -> Result<Event> {
        info!("Signing event of kind {}", event.kind);

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

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
        debug!("Getting public key");

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let pubkey = signer.get_public_key()?;
        debug!("Retrieved public key: {}", pubkey);

        Ok(pubkey)
    }

    /// Sets the current signer
    fn set_signer(&self, signer_type: SignerType, signer_data: &str) -> Result<()> {
        info!("Setting signer: type={}", signer_type);

        let new_signer = Self::create_signer(signer_type.clone(), signer_data)?;

        // Update the current signer
        {
            let mut current_lock = self.current.lock().unwrap();
            *current_lock = Some(new_signer);
        }

        // Update the signer type
        {
            let mut type_lock = self.signer_type.lock().unwrap();
            *type_lock = Some(signer_type);
        }

        info!("Signer changed successfully");
        Ok(())
    }

    /// Gets the current signer type
    fn get_signer_type(&self) -> Option<SignerType> {
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
        info!(
            "Encrypting message using NIP-04 for recipient: {}",
            recipient_pubkey
        );

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let encrypted = signer.nip04_encrypt(recipient_pubkey, plaintext)?;
        debug!("Successfully encrypted message using NIP-04");

        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-04
    fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        debug!(
            "Decrypting message using NIP-04 from sender: {}",
            sender_pubkey
        );

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let decrypted = signer.nip04_decrypt(sender_pubkey, ciphertext)?;
        debug!("Successfully decrypted message using NIP-04");

        Ok(decrypted)
    }

    /// Encrypts a message for a recipient using NIP-44
    fn nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<String> {
        info!(
            "Encrypting message using NIP-44 for recipient: {}",
            recipient_pubkey
        );

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let encrypted = signer.nip44_encrypt(recipient_pubkey, plaintext)?;
        debug!("Successfully encrypted message using NIP-44");

        Ok(encrypted)
    }

    /// Decrypts a message from a sender using NIP-44
    fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String> {
        debug!(
            "Decrypting message using NIP-44 from sender: {}",
            sender_pubkey
        );

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let decrypted = signer.nip44_decrypt(sender_pubkey, ciphertext)?;
        debug!("Successfully decrypted message using NIP-44");

        Ok(decrypted)
    }
}

impl Default for SignerManagerImpl {
    fn default() -> Self {
        Self::new()
    }
}

unsafe impl Send for SignerManagerImpl {}
unsafe impl Sync for SignerManagerImpl {}

/// WASM bindings for the signer manager
pub struct WasmSignerManager {
    inner: SignerManagerImpl,
}

impl WasmSignerManager {
    pub fn new() -> Self {
        Self {
            inner: SignerManagerImpl::new(),
        }
    }

    pub fn sign_event(&self, template: &Template) -> Result<Event, JsValue> {
        self.inner
            .sign_event(template)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn get_public_key(&self) -> Result<String, JsValue> {
        self.inner
            .get_public_key()
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn set_signer(&self, signer_type: &str, data: Option<&str>) -> Result<(), JsValue> {
        let signer_type_enum: SignerType = signer_type
            .parse()
            .map_err(|e| JsValue::from_str(&format!("Invalid signer type: {}", e)))?;
        let signer_data = data.unwrap_or("");

        self.inner
            .set_signer(signer_type_enum, signer_data)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn has_signer(&self) -> bool {
        self.inner.has_signer()
    }

    pub fn nip04_encrypt(
        &self,
        recipient_pubkey: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        self.inner
            .nip04_encrypt(recipient_pubkey, plaintext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String, JsValue> {
        self.inner
            .nip04_decrypt(sender_pubkey, ciphertext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn nip44_encrypt(
        &self,
        recipient_pubkey: &str,
        plaintext: &str,
    ) -> Result<String, JsValue> {
        self.inner
            .nip44_encrypt(recipient_pubkey, plaintext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<String, JsValue> {
        self.inner
            .nip44_decrypt(sender_pubkey, ciphertext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
