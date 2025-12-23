use js_sys::Promise;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

#[wasm_bindgen]
extern "C" {
    /// This function is implemented in the worker's JS entry point (signer/index.ts).
    /// It bridges the worker to the main thread's window.nostr extension.
    #[wasm_bindgen(js_name = callExtension)]
    fn call_extension(op: &str, payload: JsValue) -> Promise;
}

/// NIP-07 signer that communicates with a browser extension via a JS proxy.
/// This allows the Rust code running in a Web Worker to access window.nostr.
pub struct Nip07Signer;

impl Nip07Signer {
    /// Create a new NIP-07 signer handle.
    pub fn new() -> Self {
        Self
    }

    /// Get public key via `window.nostr.getPublicKey()`.
    pub async fn get_public_key(&self) -> Result<String, JsValue> {
        let promise = call_extension("getPublicKey", JsValue::NULL);
        let val = JsFuture::from(promise).await?;
        val.as_string()
            .ok_or_else(|| JsValue::from_str("NIP-07: getPublicKey returned non-string"))
    }

    /// Sign an event template via `window.nostr.signEvent(template)`.
    pub async fn sign_event(&self, template_json: &str) -> Result<serde_json::Value, JsValue> {
        let js_val = JsValue::from_str(template_json);
        let promise = call_extension("signEvent", js_val);
        let signed = JsFuture::from(promise).await?;
        serde_wasm_bindgen::from_value(signed).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// NIP-04 Encrypt via `window.nostr.nip04.encrypt(pubkey, plaintext)`.
    pub async fn nip04_encrypt(&self, pubkey: &str, plaintext: &str) -> Result<String, JsValue> {
        let payload = js_sys::Object::new();
        js_sys::Reflect::set(&payload, &"pubkey".into(), &JsValue::from_str(pubkey))?;
        js_sys::Reflect::set(&payload, &"plaintext".into(), &JsValue::from_str(plaintext))?;

        let promise = call_extension("nip04Encrypt", payload.into());
        let res = JsFuture::from(promise).await?;
        res.as_string()
            .ok_or_else(|| JsValue::from_str("NIP-07: nip04.encrypt returned non-string"))
    }

    /// NIP-04 Decrypt via `window.nostr.nip04.decrypt(pubkey, ciphertext)`.
    pub async fn nip04_decrypt(&self, pubkey: &str, ciphertext: &str) -> Result<String, JsValue> {
        // info log here
        info!(
            "nip04_decrypt called with pubkey: {}, ciphertext: {}",
            pubkey, ciphertext
        );
        let payload = js_sys::Object::new();
        js_sys::Reflect::set(&payload, &"pubkey".into(), &JsValue::from_str(pubkey))?;
        js_sys::Reflect::set(
            &payload,
            &"ciphertext".into(),
            &JsValue::from_str(ciphertext),
        )?;

        let promise = call_extension("nip04Decrypt", payload.into());
        let res = JsFuture::from(promise).await?;
        res.as_string()
            .ok_or_else(|| JsValue::from_str("NIP-07: nip04.decrypt returned non-string"))
    }

    /// NIP-44 Encrypt via `window.nostr.nip44.encrypt(pubkey, plaintext)`.
    pub async fn nip44_encrypt(&self, pubkey: &str, plaintext: &str) -> Result<String, JsValue> {
        let payload = js_sys::Object::new();
        js_sys::Reflect::set(&payload, &"pubkey".into(), &JsValue::from_str(pubkey))?;
        js_sys::Reflect::set(&payload, &"plaintext".into(), &JsValue::from_str(plaintext))?;

        let promise = call_extension("nip44Encrypt", payload.into());
        let res = JsFuture::from(promise).await?;
        res.as_string()
            .ok_or_else(|| JsValue::from_str("NIP-07: nip44.encrypt returned non-string"))
    }

    /// NIP-44 Decrypt via `window.nostr.nip44.decrypt(pubkey, ciphertext)`.
    pub async fn nip44_decrypt(&self, pubkey: &str, ciphertext: &str) -> Result<String, JsValue> {
        let payload = js_sys::Object::new();
        js_sys::Reflect::set(&payload, &"pubkey".into(), &JsValue::from_str(pubkey))?;
        js_sys::Reflect::set(
            &payload,
            &"ciphertext".into(),
            &JsValue::from_str(ciphertext),
        )?;

        let promise = call_extension("nip44Decrypt", payload.into());
        let res = JsFuture::from(promise).await?;
        res.as_string()
            .ok_or_else(|| JsValue::from_str("NIP-07: nip44.decrypt returned non-string"))
    }

    /// NIP-04 decrypt when both participants are provided (sender/recipient).
    /// Chooses the correct peer based on our pubkey and delegates to nip04_decrypt.
    pub async fn nip04_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let my_pubkey = self.get_public_key().await?;
        let peer_hex = if my_pubkey == sender_pubkey_hex {
            recipient_pubkey_hex
        } else {
            sender_pubkey_hex
        };
        self.nip04_decrypt(peer_hex, ciphertext).await
    }

    /// NIP-44 decrypt when both participants are provided (sender/recipient).
    /// Chooses the correct peer based on our pubkey and delegates to nip44_decrypt.
    pub async fn nip44_decrypt_between(
        &self,
        sender_pubkey_hex: &str,
        recipient_pubkey_hex: &str,
        ciphertext: &str,
    ) -> Result<String, JsValue> {
        let my_pubkey = self.get_public_key().await?;
        let peer_hex = if my_pubkey == sender_pubkey_hex {
            recipient_pubkey_hex
        } else {
            sender_pubkey_hex
        };
        self.nip44_decrypt(peer_hex, ciphertext).await
    }
}
