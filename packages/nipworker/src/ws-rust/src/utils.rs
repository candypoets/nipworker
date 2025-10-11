//! Utility functions for JS interop in a worker context.
//! Adapted for ws-rust; minimal bindings.

use js_sys::{Array, Object, Promise, Reflect, Uint8Array, JSON};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::IdbFactory;
use web_sys::IdbOpenDbRequest;
use web_sys::IdbRequest;

/// postMessage binding (for worker communication if needed)
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = self, js_name = postMessage)]
    pub fn post_message_raw(data: &JsValue);
}

/// Parse JSON (but we avoid it; kept for utils)
pub fn parse_json(json_str: &str) -> Result<JsValue, JsValue> {
    JSON::parse(json_str)
}

/// Stringify JSON (avoid where possible)
pub fn stringify_json(value: &JsValue) -> Result<String, JsValue> {
    let s = JSON::stringify(value)?;
    s.as_string()
        .ok_or_else(|| JsValue::from_str("JSON.stringify did not return a string"))
}

/// Post message to main thread (if in worker)
pub fn post_worker_message(message: &JsValue) {
    post_message_raw(message);
}

/// Create a plain JS object: {}
pub fn create_object() -> JsValue {
    Object::new().into()
}

/// Set a property on an object: obj[key] = value
pub fn set_property(obj: &JsValue, key: &str, value: &JsValue) -> Result<(), JsValue> {
    Reflect::set(obj, &JsValue::from_str(key), value).map(|_| ())
}

/// Create a Uint8Array view over a Rust slice (zero-copy)
pub fn create_uint8_array_from_slice(data: &[u8]) -> JsValue {
    Uint8Array::from(data).into()
}

/// Create an Array: []
pub fn create_array() -> JsValue {
    Array::new().into()
}

/// Push into an Array
pub fn array_push(array: &JsValue, item: &JsValue) -> Result<u32, JsValue> {
    let arr: Array = array.clone().unchecked_into();
    Ok(arr.push(item))
}

/// Macro for JS object construction
#[macro_export]
macro_rules! js_object {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let obj = $crate::utils::js_interop::create_object();
        $(
            let _ = $crate::utils::js_interop::set_property(&obj, $key, &JsValue::from($value));
        )*
        obj
    }};
}

/// JSON helpers (avoid in core logic)
pub struct JsonOps;

impl JsonOps {
    pub fn parse_safe(json_str: &str) -> Result<JsValue, String> {
        parse_json(json_str).map_err(|e| format!("JSON parse error: {:?}", e))
    }

    pub fn stringify_safe(value: &JsValue) -> Result<String, String> {
        stringify_json(value).map_err(|e| format!("JSON stringify error: {:?}", e))
    }
}

/// Convert &[u8] to Uint8Array (zero-copy)
pub fn uint8array_from_slice(data: &[u8]) -> Uint8Array {
    Uint8Array::from(data)
}

// Sleep helper for polling (async timeout)
pub fn sleep_ms(ms: u32) -> Promise {
    let window = web_sys::window().expect("No window");
    let promise = js_sys::Promise::new(&mut |resolve, _| {
        window
            .set_timeout_with_callback_and_timeout_and_arguments_0(resolve, ms as i32)
            .expect("set_timeout failed");
    });
    promise
}
