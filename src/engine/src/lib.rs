use futures::channel::mpsc;
use futures::StreamExt;
use js_sys::{ArrayBuffer, Uint8Array};
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::traits::Signer;
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

fn into_dyn_signer(signer: Arc<LocalSigner>) -> Arc<dyn Signer> {
    signer
}

/// WASM-facing engine worker that hosts the full NostrEngine in a single thread.
#[wasm_bindgen]
pub struct NipworkerEngine {
    engine: Rc<NostrEngine>,
    port: MessagePort,
    signer: Arc<LocalSigner>,
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
        let signer_for_engine = into_dyn_signer(Arc::clone(&signer));

        let (event_tx, mut event_rx) = mpsc::channel::<(String, Vec<u8>)>(256);
        let port_for_events = port.clone();

        // Forward engine events back to main thread via MessagePort.
        // Prepend a 4-byte little-endian length so the TS side can use
        // ArrayBufferReader.writeBatchedData directly.
        spawn_local(async move {
            while let Some((sub_id, bytes)) = event_rx.next().await {
                let mut batched = Vec::with_capacity(4 + bytes.len());
                batched.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
                batched.extend_from_slice(&bytes);
                let obj = js_sys::Object::new();
                let _ = js_sys::Reflect::set(&obj, &"subId".into(), &sub_id.into());
                let arr = Uint8Array::new_with_length(batched.len() as u32);
                arr.copy_from(&batched);
                let _ = js_sys::Reflect::set(&obj, &"data".into(), &arr.into());
                let _ = port_for_events.post_message(&obj);
            }
        });

        let engine = Rc::new(NostrEngine::new(transport, storage, signer_for_engine, event_tx));
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

        Self { engine, port, signer }
    }

    /// Direct path: set a private key signer.
    #[wasm_bindgen(js_name = "setPrivateKey")]
    pub fn set_private_key(&self, secret: String) -> Result<(), JsValue> {
        info!("[nipworker-engine] set_private_key called");
        self.signer
            .set_private_key(&secret)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }

    /// Wake the engine (e.g., after returning from background).
    pub fn wake(&self) {
        info!("[nipworker-engine] wake called");
    }
}
