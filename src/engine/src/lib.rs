use futures::channel::mpsc;
use futures::StreamExt;
use js_sys::{ArrayBuffer, Uint8Array};
use nipworker_core::service::engine::NostrEngine;
use std::rc::Rc;
use std::sync::Arc;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, MessagePort};

mod signer;
mod storage;
mod transport;

use signer::LocalSigner;
use storage::MemoryStorage;
use transport::WebSocketTransport;

/// WASM-facing engine worker that hosts the full NostrEngine in a single thread.
#[wasm_bindgen]
pub struct NipworkerEngine {
    engine: Rc<NostrEngine>,
    port: MessagePort,
}

#[wasm_bindgen]
impl NipworkerEngine {
    /// new(port: MessagePort)
    ///
    /// The main thread sends FlatBuffers `MainMessage` bytes through this port,
    /// and receives batched event bytes back.
    #[wasm_bindgen(constructor)]
    pub fn new(port: MessagePort) -> Self {
        console_error_panic_hook::set_once();
        info!("[nipworker-engine] Initializing WASM engine...");

        let transport = Arc::new(WebSocketTransport::new());
        let storage = Arc::new(MemoryStorage::new());
        let signer = Arc::new(LocalSigner::new());

        let (event_tx, mut event_rx) = mpsc::channel::<(String, Vec<u8>)>(256);
        let port_for_events = port.clone();

        // Forward engine events back to main thread via MessagePort
        spawn_local(async move {
            while let Some((sub_id, bytes)) = event_rx.next().await {
                // Pack into a simple JS object: { subId, data: Uint8Array }
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"subId".into(), &sub_id.into());
                let arr = Uint8Array::new_with_length(bytes.len() as u32);
                arr.copy_from(&bytes);
                let _ = js_sys::Reflect::set(&obj, &"data".into(), &arr.into());
                let _ = port_for_events.post_message(&obj);
            }
        });

        let engine = Rc::new(NostrEngine::new(transport, storage, signer, event_tx));
        let engine_for_messages = engine.clone();

        // Set up onmessage handler
        let closure = Closure::wrap(Box::new(move |event: MessageEvent| {
            let bytes = if let Ok(data) = event.data().dyn_into::<ArrayBuffer>() {
                let array = Uint8Array::new(&data);
                array.to_vec()
            } else if let Ok(data) = event.data().dyn_into::<Uint8Array>() {
                data.to_vec()
            } else if let Ok(obj) = event.data().dyn_into::<js_sys::Object>() {
                // Support { serializedMessage: Uint8Array } format
                if let Ok(serialized) = js_sys::Reflect::get(&obj, &"serializedMessage".into()) {
                    if let Ok(uint8) = serialized.dyn_into::<Uint8Array>() {
                        uint8.to_vec()
                    } else {
                        return;
                    }
                } else {
                    return;
                }
            } else {
                return;
            };

            let engine = engine_for_messages.clone();
            spawn_local(async move {
                if let Err(e) = engine.handle_message(&bytes).await {
                    tracing::warn!("[nipworker-engine] handle_message error: {}", e);
                }
            });
        }) as Box<dyn FnMut(MessageEvent)>);

        port.set_onmessage(Some(closure.as_ref().unchecked_ref()));
        closure.forget();

        Self { engine, port }
    }

    /// Direct path: set a private key signer.
    #[wasm_bindgen(js_name = "setPrivateKey")]
    pub fn set_private_key(&self, _secret: String) -> Result<(), JsValue> {
        // In a full implementation we'd need to access the signer inside the engine.
        // For now this is a stub that logs the request.
        info!("[nipworker-engine] set_private_key called");
        // TODO: expose signer setter on NostrEngine
        Ok(())
    }

    /// Wake the engine (e.g., after returning from background).
    pub fn wake(&self) {
        info!("[nipworker-engine] wake called");
    }
}
