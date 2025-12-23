use crate::signers::{nip04, nip44, nip44::ConversationKey};
use shared::types::{Keys, PublicKey};
use tracing::warn;
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

    pub fn encrypt_for_remote(&self, plaintext: &str) -> Result<String, JsValue> {
        let remote_pk = PublicKey::from_hex(&self.remote_signer_pubkey)
            .map_err(|e| JsValue::from_str(&format!("pk: {}", e)))?;
        let secret = self
            .client_keys
            .secret_key()
            .map_err(|e| JsValue::from_str(&format!("secret key: {}", e)))?;

        if self.use_nip44 {
            let conv = ConversationKey::derive(secret, &remote_pk)
                .map_err(|e| JsValue::from_str(&format!("nip44 derive: {}", e)))?;
            match nip44::encrypt(plaintext, &conv) {
                Ok(ct) => return Ok(ct),
                Err(e) => {
                    warn!("[nip46] nip44 encrypt failed, trying nip04: {}", e);
                }
            }
        }

        // NIP-04 fallback
        nip04::encrypt(secret, &remote_pk, plaintext)
            .map_err(|e| JsValue::from_str(&format!("nip04 encrypt: {}", e)))
    }

    pub fn decrypt_from_remote(&self, ciphertext: &str) -> Result<String, JsValue> {
        let remote_pk = PublicKey::from_hex(&self.remote_signer_pubkey)
            .map_err(|e| JsValue::from_str(&format!("pk: {}", e)))?;
        let secret = self
            .client_keys
            .secret_key()
            .map_err(|e| JsValue::from_str(&format!("secret key: {}", e)))?;

        if self.use_nip44 {
            let conv = ConversationKey::derive(secret, &remote_pk)
                .map_err(|e| JsValue::from_str(&format!("nip44 derive: {}", e)))?;
            match nip44::decrypt(ciphertext, &conv) {
                Ok(pt) => return Ok(pt),
                Err(e) => {
                    warn!("[nip46] nip44 decrypt failed, trying nip04: {}", e);
                }
            }
        }

        nip04::decrypt(secret, &remote_pk, ciphertext)
            .map_err(|e| JsValue::from_str(&format!("nip04 decrypt: {}", e)))
    }
}
