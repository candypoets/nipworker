use js_sys::Promise;
use js_sys::Uint8Array;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::prelude::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use web_sys::IdbFactory;
use web_sys::IdbOpenDbRequest;
use web_sys::IdbRequest;

#[wasm_bindgen]
extern "C" {
    // self.indexedDB
    #[wasm_bindgen(js_namespace = self, js_name = indexedDB)]
    static INDEXED_DB: JsValue;
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
