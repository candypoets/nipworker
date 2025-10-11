use js_sys::Promise;
use js_sys::Uint8Array;
use wasm_bindgen::prelude::*;

/// JS must define these global functions before loading WASM.
/// E.g., in index.ts: window.create_websocket = (url) => new WebSocket(url); etc.

#[wasm_bindgen]
extern "C" {
    // Create WS and return the WS object (JsValue)
    #[wasm_bindgen(js_name = create_websocket)]
    fn create_websocket_js(url: &str) -> JsValue;

    // Send message on WS (Promise for completion)
    #[wasm_bindgen(js_name = ws_send)]
    fn ws_send_js(ws: &JsValue, data: &str) -> Promise;

    // Close WS (Promise)
    #[wasm_bindgen(js_name = ws_close)]
    fn ws_close_js(ws: &JsValue) -> Promise;

    // JS calls this Rust function for incoming messages
    #[wasm_bindgen(js_name = handle_incoming_message)]
    pub fn handle_incoming_message(url: String, data: Uint8Array);
}

/// Init: Expose handle_incoming_message to global (no manual registration needed)
#[wasm_bindgen]
pub fn init_ws_interop() {
    // Already exposed via extern; JS can call directly
    // Optional: Log or setup if needed
}
