//! Utility functions for JS interop in a worker context.
//! Keep bindings minimal and rely on js-sys/web-sys where stable.

use js_sys::Promise;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

use js_sys::{global, Array, Object, Reflect, Uint8Array, JSON};
use web_sys::IdbFactory;
use web_sys::IdbOpenDbRequest;
use web_sys::IdbRequest;

/// postMessage(self.postMessage) binding
#[wasm_bindgen]
extern "C" {
    // self.postMessage(message)
    #[wasm_bindgen(js_namespace = self, js_name = postMessage)]
    pub fn post_message_raw(data: &JsValue);

    // self.indexedDB
    #[wasm_bindgen(js_namespace = self, js_name = indexedDB)]
    static INDEXED_DB: JsValue;
}

/// Get IndexedDB from the worker global scope.
pub fn get_indexed_db() -> Result<JsValue, JsValue> {
    let g = global();
    Reflect::get(&g, &JsValue::from_str("indexedDB"))
}

/// Parse JSON (safe wrapper over js_sys::JSON)
pub fn parse_json(json_str: &str) -> Result<JsValue, JsValue> {
    JSON::parse(json_str)
}

/// Stringify JSON (safe wrapper over js_sys::JSON)
pub fn stringify_json(value: &JsValue) -> Result<String, JsValue> {
    let s = JSON::stringify(value)?;
    s.as_string()
        .ok_or_else(|| JsValue::from_str("JSON.stringify did not return a string"))
}

/// Post message to main thread
pub fn post_worker_message(message: &JsValue) {
    // Call self.postMessage(message)
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

/// Create a Uint8Array view over a Rust slice (no copy where possible)
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

/// Macro to reduce boilerplate for JS object construction:
/// let obj = js_object!{ "a" => 1, "b" => "two" };
#[macro_export]
macro_rules! js_object {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let obj = $crate::utils::js_interop::create_object();
        $(
            // .into() is applied to the right-hand side, so literals work
            let _ = $crate::utils::js_interop::set_property(&obj, $key, &JsValue::from($value));
        )*
        obj
    }};
}

/// JSON helpers with string error messages (useful in non-wasm contexts too)
pub struct JsonOps;

impl JsonOps {
    pub fn parse_safe(json_str: &str) -> Result<JsValue, String> {
        parse_json(json_str).map_err(|e| format!("JSON parse error: {:?}", e))
    }

    pub fn stringify_safe(value: &JsValue) -> Result<String, String> {
        stringify_json(value).map_err(|e| format!("JSON stringify error: {:?}", e))
    }
}

pub fn get_idb_factory() -> Result<IdbFactory, JsValue> {
    INDEXED_DB
        .clone()
        .dyn_into::<IdbFactory>()
        .map_err(|_| JsValue::from_str("indexedDB not available or wrong type"))
}

pub fn idb_open_request_promise(open_request: &IdbOpenDbRequest) -> Promise {
    let open_request = open_request.clone();
    Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        // Clone for each closure that needs them
        let resolve_for_success = resolve.clone();
        let reject_for_success = reject.clone();
        let reject_for_error = reject.clone();

        let req_for_success = open_request.clone();
        let on_success = Closure::once(move || match req_for_success.result() {
            Ok(db) => {
                let _ = resolve_for_success.call1(&JsValue::UNDEFINED, &db);
            }
            Err(_) => {
                let _ = reject_for_success.call1(
                    &JsValue::UNDEFINED,
                    &JsValue::from_str("open_request.result() failed"),
                );
            }
        });

        let on_error = Closure::once(move || {
            let _ = reject_for_error.call1(
                &JsValue::UNDEFINED,
                &JsValue::from_str("Failed to open database"),
            );
        });

        open_request.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
        open_request.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_success.forget();
        on_error.forget();
    })
}

pub fn idb_request_promise(request: &IdbRequest) -> Promise {
    let request = request.clone();
    Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        // Clone for each closure that needs them
        let resolve_for_success = resolve.clone();
        let reject_for_success = reject.clone();
        let reject_for_error = reject.clone();

        let req_for_success = request.clone();
        let on_success = Closure::once(move || match req_for_success.result() {
            Ok(result) => {
                let _ = resolve_for_success.call1(&JsValue::UNDEFINED, &result);
            }
            Err(_) => {
                let _ = reject_for_success.call1(
                    &JsValue::UNDEFINED,
                    &JsValue::from_str("request.result() failed"),
                );
            }
        });

        let on_error = Closure::once(move || {
            let _ = reject_for_error.call1(
                &JsValue::UNDEFINED,
                &JsValue::from_str("IndexedDB request failed"),
            );
        });

        request.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
        request.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_success.forget();
        on_error.forget();
    })
}

/// Convert &[u8] to Uint8Array
pub fn uint8array_from_slice(data: &[u8]) -> Uint8Array {
    Uint8Array::from(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_worker);

    #[wasm_bindgen_test]
    fn test_create_object() {
        let obj = create_object();
        assert!(obj.is_object());
    }

    #[wasm_bindgen_test]
    fn test_json_operations() {
        let test_json = r#"{"test": "value"}"#;
        let parsed = JsonOps::parse_safe(test_json).unwrap();
        let stringified = JsonOps::stringify_safe(&parsed).unwrap();
        assert!(stringified.contains("test"));
    }

    #[wasm_bindgen_test]
    fn test_uint8_array() {
        let data = vec![1, 2, 3, 4];
        let array = create_uint8_array_from_slice(&data);
        assert!(array.is_object());
    }
}
