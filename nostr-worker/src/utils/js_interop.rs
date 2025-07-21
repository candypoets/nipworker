//! Utility functions to reduce js-sys dependencies
//! This module provides lightweight alternatives to some js-sys APIs

use wasm_bindgen::prelude::*;
use web_sys::{DedicatedWorkerGlobalScope, WorkerGlobalScope};

/// Lightweight alternative to js_sys::global() for worker context
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["self"])]
    static WORKER_SELF: JsValue;

    #[wasm_bindgen(js_name = "indexedDB", js_namespace = ["self"])]
    static INDEXED_DB: JsValue;

    #[wasm_bindgen(js_name = "postMessage", js_namespace = ["self"])]
    fn post_message_raw(data: &JsValue);

    #[wasm_bindgen(js_name = "JSON", js_namespace = ["JSON"])]
    type JsonAPI;

    #[wasm_bindgen(method, js_name = "parse")]
    fn parse(this: &JsonAPI, text: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(method, js_name = "stringify")]
    fn stringify(this: &JsonAPI, value: &JsValue) -> Result<JsValue, JsValue>;
}

/// Get the worker global scope without js_sys::global()
pub fn get_worker_global() -> Result<JsValue, JsValue> {
    Ok(WORKER_SELF.clone())
}

/// Get IndexedDB directly without going through js_sys
pub fn get_indexed_db() -> Result<JsValue, JsValue> {
    Ok(INDEXED_DB.clone())
}

/// Parse JSON without js_sys::JSON
pub fn parse_json(json_str: &str) -> Result<JsValue, JsValue> {
    let json_api = JsValue::from("JSON").into();
    JsonAPI::parse(&json_api, json_str)
}

/// Stringify object without js_sys::JSON
pub fn stringify_json(value: &JsValue) -> Result<String, JsValue> {
    let json_api = JsValue::from("JSON").into();
    JsonAPI::stringify(&json_api, value)?
        .as_string()
        .ok_or_else(|| JsValue::from_str("Failed to convert to string"))
}

/// Post message to main thread without js_sys overhead
pub fn post_worker_message(message: &JsValue) {
    post_message_raw(message);
}

/// Create a simple object without js_sys::Object
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "Object")]
    type SimpleObject;

    #[wasm_bindgen(constructor)]
    fn new() -> SimpleObject;
}

pub fn create_object() -> JsValue {
    SimpleObject::new().into()
}

/// Set object property without js_sys::Reflect
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "Object")]
    type ObjectUtils;

    #[wasm_bindgen(static_method_of = ObjectUtils, js_name = "defineProperty")]
    fn define_property(obj: &JsValue, prop: &str, descriptor: &JsValue);
}

pub fn set_property(obj: &JsValue, key: &str, value: &JsValue) {
    let descriptor = create_object();
    // Simple assignment without full descriptor
    js_sys::Reflect::set(obj, &JsValue::from(key), value).unwrap();
}

/// Create Uint8Array without js_sys
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "Uint8Array")]
    type FastUint8Array;

    #[wasm_bindgen(constructor)]
    fn new(length: u32) -> FastUint8Array;

    #[wasm_bindgen(constructor)]
    fn from_buffer(buffer: &[u8]) -> FastUint8Array;

    #[wasm_bindgen(method)]
    fn set(this: &FastUint8Array, array: &[u8], offset: Option<u32>);
}

pub fn create_uint8_array_from_slice(data: &[u8]) -> JsValue {
    FastUint8Array::from_buffer(data).into()
}

/// Optimized Array operations
#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = "Array")]
    type FastArray;

    #[wasm_bindgen(constructor)]
    fn new() -> FastArray;

    #[wasm_bindgen(method)]
    fn push(this: &FastArray, item: &JsValue);
}

pub fn create_array() -> JsValue {
    FastArray::new().into()
}

pub fn array_push(array: &JsValue, item: &JsValue) {
    let fast_array: &FastArray = array.unchecked_ref();
    fast_array.push(item);
}

/// Worker context utilities
pub struct WorkerContext;

impl WorkerContext {
    /// Get the worker global scope in a type-safe way
    pub fn global() -> Result<WorkerGlobalScope, JsValue> {
        get_worker_global()?
            .dyn_into::<WorkerGlobalScope>()
            .map_err(|_| JsValue::from_str("Not in worker context"))
    }

    /// Get dedicated worker global scope
    pub fn dedicated_global() -> Result<DedicatedWorkerGlobalScope, JsValue> {
        get_worker_global()?
            .dyn_into::<DedicatedWorkerGlobalScope>()
            .map_err(|_| JsValue::from_str("Not in dedicated worker context"))
    }

    /// Post a simple message with type and payload
    pub fn post_typed_message(event_type: &str, payload: Option<&JsValue>) -> Result<(), JsValue> {
        let message = create_object();
        set_property(&message, "type", &JsValue::from_str(event_type));

        if let Some(payload) = payload {
            set_property(&message, "payload", payload);
        }

        post_worker_message(&message);
        Ok(())
    }
}

/// Macro to reduce boilerplate for JS interop
#[macro_export]
macro_rules! js_object {
    ($($key:expr => $value:expr),* $(,)?) => {{
        let obj = $crate::utils::js_interop::create_object();
        $(
            $crate::utils::js_interop::set_property(&obj, $key, &$value.into());
        )*
        obj
    }};
}

/// Performance-optimized JSON operations
pub struct JsonOps;

impl JsonOps {
    /// Parse JSON with error handling
    pub fn parse_safe(json_str: &str) -> Result<JsValue, String> {
        parse_json(json_str).map_err(|e| format!("JSON parse error: {:?}", e))
    }

    /// Stringify with error handling
    pub fn stringify_safe(value: &JsValue) -> Result<String, String> {
        stringify_json(value).map_err(|e| format!("JSON stringify error: {:?}", e))
    }
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
