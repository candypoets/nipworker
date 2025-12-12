use js_sys::{Object, Promise, Reflect};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// NIP-07 signer that talks to a browser provider exposed at `window.nostr`.
/// This module is browser/wasm-only and avoids async_trait; methods are plain async fns.
pub struct Nip07Signer;

impl Nip07Signer {
    /// Create a new NIP-07 signer handle.
    pub fn new() -> Self {
        Self
    }

    /// Resolve `window.nostr` as a JS object.
    fn nostr_obj() -> Result<Object, JsValue> {
        let win = web_sys::window().ok_or_else(|| JsValue::from_str("NIP-07: no window"))?;
        let nostr = Reflect::get(&win, &JsValue::from_str("nostr"))
            .map_err(|_| JsValue::from_str("NIP-07: window.nostr missing"))?;
        nostr
            .dyn_into::<Object>()
            .map_err(|_| JsValue::from_str("NIP-07: window.nostr is not an object"))
    }

    /// Get public key via `window.nostr.getPublicKey()`.
    pub async fn get_public_key(&self) -> Result<String, JsValue> {
        let nostr = Self::nostr_obj()?;
        let f = Reflect::get(&nostr, &JsValue::from_str("getPublicKey"))
            .map_err(|_| JsValue::from_str("NIP-07: nostr.getPublicKey missing"))?;
        let f = f
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("NIP-07: getPublicKey is not a function"))?;

        let p = f
            .call0(&nostr)
            .map_err(|_| JsValue::from_str("NIP-07: getPublicKey call failed"))?;
        let val = JsFuture::from(
            p.dyn_into::<Promise>()
                .map_err(|_| JsValue::from_str("NIP-07: getPublicKey did not return a Promise"))?,
        )
        .await
        .map_err(|_| JsValue::from_str("NIP-07: getPublicKey Promise rejected"))?;

        val.as_string()
            .ok_or_else(|| JsValue::from_str("NIP-07: getPublicKey returned non-string"))
    }

    /// Sign an event template (as serde_json::Value) via `window.nostr.signEvent(template)`.
    /// Returns the signed event as a serde_json::Value.
    pub async fn sign_event(&self, template: &str) -> Result<serde_json::Value, JsValue> {
        let nostr = Self::nostr_obj()?;
        let f = Reflect::get(&nostr, &JsValue::from_str("signEvent"))
            .map_err(|_| JsValue::from_str("NIP-07: nostr.signEvent missing"))?;
        let f = f
            .dyn_into::<js_sys::Function>()
            .map_err(|_| JsValue::from_str("NIP-07: signEvent is not a function"))?;

        let js_evt = serde_wasm_bindgen::to_value(template)
            .map_err(|e| JsValue::from_str(&e.to_string()))?;

        let p = f
            .call1(&nostr, &js_evt)
            .map_err(|_| JsValue::from_str("NIP-07: signEvent call failed"))?;
        let signed = JsFuture::from(
            p.dyn_into::<Promise>()
                .map_err(|_| JsValue::from_str("NIP-07: signEvent did not return a Promise"))?,
        )
        .await
        .map_err(|_| JsValue::from_str("NIP-07: signEvent Promise rejected"))?;

        serde_wasm_bindgen::from_value(signed).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
