use tracing::{info, warn};

/// Parser-facing client for the signer service.
/// This is a platform-agnostic stub. The WASM wrapper injects the real
/// MessageChannel-based implementation.
pub struct CryptoClient;

impl CryptoClient {
    pub fn new() -> Self {
        info!("[crypto-client] initialized (stub)");
        Self
    }

    /// Core generic call using raw string protocol.
    pub async fn call_raw(
        &self,
        op: &str,
        payload: Option<&str>,
        _pubkey: Option<&str>,
        _sender_pubkey: Option<&str>,
        _recipient_pubkey: Option<&str>,
    ) -> Result<String, String> {
        warn!("[crypto-client] stub call_raw invoked for op={}", op);
        if let Some(p) = payload {
            Ok(p.to_string())
        } else {
            Err(format!("CryptoClient stub: op {} not implemented", op))
        }
    }

    pub async fn get_public_key(&self) -> Result<String, String> {
        Err("CryptoClient stub: get_public_key not implemented".to_string())
    }

    pub async fn sign_event(&self, template: String) -> Result<String, String> {
        Ok(template)
    }

    pub async fn nip04_encrypt(
        &self,
        _recipient_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        Ok(plaintext.to_string())
    }

    pub async fn nip44_encrypt(
        &self,
        _recipient_pubkey_hex: &str,
        plaintext: &str,
    ) -> Result<String, String> {
        Ok(plaintext.to_string())
    }

    pub async fn nip04_decrypt(
        &self,
        _sender_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        Ok(ciphertext.to_string())
    }

    pub async fn nip44_decrypt(
        &self,
        _sender_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        Ok(ciphertext.to_string())
    }

    pub async fn nip04_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        info!(
            "[crypto-client] nip04_decrypt_between sender={} recipient={} ciphertext_len={}",
            sender_pubkey_hex,
            recipient_pubkey_hex,
            ciphertext.len()
        );
        Ok(ciphertext.to_string())
    }

    pub async fn nip44_decrypt_between(
        &self,
        _sender_pubkey_hex: &str,
        _recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, String> {
        Ok(ciphertext.to_string())
    }

    pub async fn verify_proof(
        &self,
        _proof_json: String,
        _mint_keys_json: String,
    ) -> Result<String, String> {
        Ok(String::new())
    }
}

impl Default for CryptoClient {
    fn default() -> Self {
        Self::new()
    }
}
