use crate::signers::{nip04, nip44, nip44::ConversationKey};
use shared::types::{Keys, PublicKey};
use tracing::error;
use wasm_bindgen::prelude::*;

pub struct Crypto {
    client_keys: Keys,
    remote_signer_pubkey: String,
    use_nip44: bool,
}

impl Crypto {
    pub fn new(client_keys: Keys, remote_signer_pubkey: String, use_nip44: bool) -> Self {
        Self {
            client_keys,
            remote_signer_pubkey,
            use_nip44,
        }
    }

    /// Update the remote signer pubkey (used after QR discovery)
    pub fn set_remote_signer_pubkey(&mut self, pubkey: String) {
        self.remote_signer_pubkey = pubkey;
    }

    pub fn encrypt_for_remote(&self, plaintext: &str) -> Result<String, JsValue> {
        if self.remote_signer_pubkey.is_empty() {
            return Err(JsValue::from_str("Remote signer pubkey not yet discovered"));
        }
        
        let remote_pk = PublicKey::from_hex(&self.remote_signer_pubkey)
            .map_err(|e| JsValue::from_str(&format!("pk: {}", e)))?;
        
        let secret = self.client_keys.secret_key()
            .map_err(|e| JsValue::from_str(&format!("secret key: {}", e)))?;

        if self.use_nip44 {
            if let Ok(conv) = ConversationKey::derive(secret, &remote_pk) {
                if let Ok(ct) = nip44::encrypt(plaintext, &conv) {
                    return Ok(ct);
                }
            }
        }

        nip04::encrypt(secret, &remote_pk, plaintext)
            .map_err(|e| JsValue::from_str(&format!("nip04 encrypt: {}", e)))
    }

    pub fn decrypt_from_remote(&self, ciphertext: &str) -> Result<String, JsValue> {
        if self.remote_signer_pubkey.is_empty() {
            return Err(JsValue::from_str("Remote signer pubkey not set"));
        }
        
        let remote_pk = PublicKey::from_hex(&self.remote_signer_pubkey)
            .map_err(|e| JsValue::from_str(&format!("pk: {}", e)))?;
        
        let secret = self.client_keys.secret_key()
            .map_err(|e| JsValue::from_str(&format!("secret key: {}", e)))?;

        if self.use_nip44 {
            if let Ok(conv) = ConversationKey::derive(secret, &remote_pk) {
                if let Ok(pt) = nip44::decrypt(ciphertext, &conv) {
                    return Ok(pt);
                }
            }
        }

        nip04::decrypt(secret, &remote_pk, ciphertext)
            .map_err(|e| JsValue::from_str(&format!("nip04 decrypt: {}", e)))
    }
}
