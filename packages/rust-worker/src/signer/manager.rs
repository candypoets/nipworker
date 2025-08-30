use crate::types::{EventTemplate, SignerMessage, SignerType};
use anyhow::Result;
use nostr::{Event, UnsignedEvent};
use rmp_serde as rmps;
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

    /// Sets a JavaScript callback function for communication with the main thread
    pub fn set_callback(&mut self, callback: js_sys::Function) {
        self.callback = Some(callback);
        debug!("JavaScript callback set for SignerManager");
    }

    /// Sends a message to JavaScript using the callback
    fn send_message(&self, message: SignerMessage) {
        if let Some(callback) = &self.callback {
            match serde_json::to_string(&message) {
                Ok(json_str) => {
                    let js_value = JsValue::from_str(&json_str);
                    if let Err(e) = callback.call1(&JsValue::NULL, &js_value) {
                        error!("Failed to call JavaScript callback: {:?}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to serialize message: {}", e);
                }
            }
        } else {
            warn!("No JavaScript callback set, cannot send message");
        }
    }

    /// Posts a message using the web worker postMessage API
    fn post_message(&self, event_type: &str, payload: Option<JsValue>) {
        let message = js_sys::Object::new();
        js_sys::Reflect::set(&message, &"type".into(), &event_type.into()).unwrap();

        if let Some(payload) = payload {
            js_sys::Reflect::set(&message, &"payload".into(), &payload).unwrap();
        }

        // Get the global scope and post message
        let global = js_sys::global();
        if let Ok(self_obj) = js_sys::Reflect::get(&global, &"self".into()) {
            if let Ok(post_message) = js_sys::Reflect::get(&self_obj, &"postMessage".into()) {
                let post_message_fn: js_sys::Function = post_message.into();
                if let Err(e) = post_message_fn.call1(&self_obj, &message) {
                    error!("Failed to post message: {:?}", e);
                }
            }
        }
    }

    /// Handles JavaScript sign event requests
    pub fn js_sign_event(&self, binary_data: &[u8]) -> Result<()> {
        debug!(
            "Received sign event request with {} bytes",
            binary_data.len()
        );

        // Deserialize the binary data using MessagePack
        let mut event: UnsignedEvent = rmps::from_slice(binary_data)
            .map_err(|e| anyhow::anyhow!("Failed to parse binary data: {}", e))?;

        // Sign the event
        self.sign_event(&mut event)?;

        // Re-encode the signed event with MessagePack
        let packed_event = rmps::to_vec_named(&event)
            .map_err(|e| anyhow::anyhow!("Failed to encode signed event: {}", e))?;

        // Create a JavaScript Uint8Array and post the message
        let uint8_array = js_sys::Uint8Array::new_with_length(packed_event.len() as u32);
        uint8_array.copy_from(&packed_event);

        self.post_message("SIGNED", Some(uint8_array.into()));

        Ok(())
    }

    /// Handles JavaScript get public key requests
    pub fn js_get_public_key(&self) -> Result<()> {
        debug!("Received get public key request");

        let pubkey = self.get_public_key()?;
        self.post_message("PUBKEY", Some(pubkey.into()));

        Ok(())
    }

    /// Handles JavaScript NIP-04 encrypt requests
    pub fn js_nip04_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<()> {
        debug!(
            "Received NIP-04 encrypt request for recipient: {}",
            recipient_pubkey
        );

        let encrypted = self.nip04_encrypt(recipient_pubkey, plaintext)?;
        self.post_message("NIP04_ENCRYPTED", Some(encrypted.into()));

        Ok(())
    }

    /// Handles JavaScript NIP-04 decrypt requests
    pub fn js_nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<()> {
        debug!(
            "Received NIP-04 decrypt request from sender: {}",
            sender_pubkey
        );

        let decrypted = self.nip04_decrypt(sender_pubkey, ciphertext)?;
        self.post_message("NIP04_DECRYPTED", Some(decrypted.into()));

        Ok(())
    }

    /// Handles JavaScript NIP-44 encrypt requests
    pub fn js_nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<()> {
        debug!(
            "Received NIP-44 encrypt request for recipient: {}",
            recipient_pubkey
        );

        let encrypted = self.nip44_encrypt(recipient_pubkey, plaintext)?;
        self.post_message("NIP44_ENCRYPTED", Some(encrypted.into()));

        Ok(())
    }

    /// Handles JavaScript NIP-44 decrypt requests
    pub fn js_nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<()> {
        debug!(
            "Received NIP-44 decrypt request from sender: {}",
            sender_pubkey
        );

        let decrypted = self.nip44_decrypt(sender_pubkey, ciphertext)?;
        self.post_message("NIP44_DECRYPTED", Some(decrypted.into()));

        Ok(())
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
    fn sign_event(&self, event: &mut UnsignedEvent) -> Result<Event> {
        info!("Signing event of kind {}", event.kind);

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let signed_event = signer.sign_event(event)?;

        debug!("Successfully signed event: {}", event.id);
        Ok(signed_event)
    }

    /// Converts an EventTemplate to an UnsignedEvent using the current signer's public key
    fn unsign_event(&self, template: EventTemplate) -> Result<UnsignedEvent> {
        info!(
            "Converting EventTemplate to UnsignedEvent for kind {}",
            template.kind
        );

        let current_lock = self.current.lock().unwrap();
        let signer = current_lock
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No signer available"))?;

        let unsigned_event = signer.unsign_event(template)?;

        debug!("Successfully created UnsignedEvent");
        Ok(unsigned_event)
    }

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
        console_error_panic_hook::set_once();

        Self {
            inner: SignerManagerImpl::new(),
        }
    }

    pub fn sign_event(&self, binary_data: &[u8]) -> Result<(), JsValue> {
        self.inner
            .js_sign_event(binary_data)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn get_public_key(&self) -> Result<(), JsValue> {
        self.inner
            .js_get_public_key()
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

    pub fn js_nip04_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<(), JsValue> {
        self.inner
            .js_nip04_encrypt(recipient_pubkey, plaintext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn js_nip04_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<(), JsValue> {
        self.inner
            .js_nip04_decrypt(sender_pubkey, ciphertext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn js_nip44_encrypt(&self, recipient_pubkey: &str, plaintext: &str) -> Result<(), JsValue> {
        self.inner
            .js_nip44_encrypt(recipient_pubkey, plaintext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn js_nip44_decrypt(&self, sender_pubkey: &str, ciphertext: &str) -> Result<(), JsValue> {
        self.inner
            .js_nip44_decrypt(sender_pubkey, ciphertext)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    #[test]
    fn test_signer_manager_creation() {
        let manager = SignerManagerImpl::new();
        assert!(!manager.has_signer());
        assert!(manager.get_signer_type().is_none());
    }

    #[test]
    fn test_set_signer() {
        let manager = SignerManagerImpl::new();

        // Set a signer with generated key
        assert!(manager.set_signer(SignerType::PrivKey, "").is_ok());
        assert!(manager.has_signer());
        assert!(matches!(
            manager.get_signer_type(),
            Some(SignerType::PrivKey)
        ));
    }

    #[test]
    fn test_sign_event() {
        let manager = SignerManagerImpl::new();
        manager.set_signer(SignerType::PrivKey, "").unwrap();

        // Create event using EventBuilder and sign it
        let event_builder = EventBuilder::new(Kind::TextNote, "Test event", vec![]);
        let pubkey = nostr::PublicKey::from_hex(manager.get_public_key().unwrap()).unwrap();
        let mut unsigned_event = event_builder.to_unsigned_event(pubkey);
        let event = manager.sign_event(&mut unsigned_event).unwrap();
        assert!(manager.sign_event(&mut unsigned_event).is_ok());
        assert!(event.verify().is_ok());
    }

    #[test]
    fn test_get_public_key() {
        let manager = SignerManagerImpl::new();
        manager.set_signer(SignerType::PrivKey, "").unwrap();

        let pubkey = manager.get_public_key().unwrap();
        assert!(!pubkey.is_empty());
        assert_eq!(pubkey.len(), 64); // hex public key length
    }

    #[test]
    fn test_nip04_encryption_decryption() {
        let manager1 = SignerManagerImpl::new();
        let manager2 = SignerManagerImpl::new();

        manager1.set_signer(SignerType::PrivKey, "").unwrap();
        manager2.set_signer(SignerType::PrivKey, "").unwrap();

        let pubkey1 = manager1.get_public_key().unwrap();
        let pubkey2 = manager2.get_public_key().unwrap();

        let message = "Secret message for NIP-04";

        // Encrypt with manager1, decrypt with manager2
        let encrypted = manager1.nip04_encrypt(&pubkey2, message).unwrap();
        let decrypted = manager2.nip04_decrypt(&pubkey1, &encrypted).unwrap();

        assert_eq!(message, decrypted);
        assert_ne!(message, encrypted);
    }

    #[test]
    fn test_nip44_encryption_decryption() {
        let manager1 = SignerManagerImpl::new();
        let manager2 = SignerManagerImpl::new();

        manager1.set_signer(SignerType::PrivKey, "").unwrap();
        manager2.set_signer(SignerType::PrivKey, "").unwrap();

        let pubkey1 = manager1.get_public_key().unwrap();
        let pubkey2 = manager2.get_public_key().unwrap();

        let message = "Secret message for NIP-44";

        // Encrypt with manager1, decrypt with manager2
        let encrypted = manager1.nip44_encrypt(&pubkey2, message).unwrap();
        let decrypted = manager2.nip44_decrypt(&pubkey1, &encrypted).unwrap();

        assert_eq!(message, decrypted);
        assert_ne!(message, encrypted);
    }

    #[test]
    fn test_encryption_without_signer() {
        let manager = SignerManagerImpl::new();

        let pubkey = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let message = "Test message";

        assert!(manager.nip04_encrypt(pubkey, message).is_err());
        assert!(manager.nip04_decrypt(pubkey, "encrypted").is_err());
        assert!(manager.nip44_encrypt(pubkey, message).is_err());
        assert!(manager.nip44_decrypt(pubkey, "encrypted").is_err());
    }

    #[test]
    fn test_no_signer_error() {
        let manager = SignerManagerImpl::new();

        assert!(manager.get_public_key().is_err());

        // Create event using EventBuilder
        let event_builder = EventBuilder::new(Kind::TextNote, "Test event", vec![]);
        let dummy_keys = Keys::generate();
        let mut event = event_builder.to_unsigned_event(dummy_keys.public_key());

        assert!(manager.sign_event(&mut event).is_err());
    }
}
