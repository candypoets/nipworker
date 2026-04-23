use futures::channel::mpsc;
use futures::StreamExt;
use js_sys::Uint8Array;
use nipworker_core::service::engine::NostrEngine;
use std::rc::Rc;
use std::sync::Arc;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

mod idb_utils;
mod ring_buffer_persist;
mod storage;
mod transport;

use storage::NostrDbStorage;
use transport::WebSocketTransport;

/// WASM-facing engine worker that hosts the full NostrEngine in a single thread.
/// Thin wrapper — all orchestration lives in the TypeScript worker.
#[wasm_bindgen]
pub struct NipworkerEngine {
	engine: Rc<NostrEngine>,
}

#[wasm_bindgen]
impl NipworkerEngine {
	/// new(on_event)
	///
	/// `on_event`: (subId: string, data: Uint8Array) => void
	#[wasm_bindgen(constructor)]
	pub fn new(on_event: js_sys::Function) -> Self {
		console_error_panic_hook::set_once();
		tracing_wasm::set_as_global_default();
		info!("[nipworker-engine] Initializing WASM engine...");

		let transport = Arc::new(WebSocketTransport::new());
		let storage = Arc::new(NostrDbStorage::new(8 * 1024 * 1024));

		// ── Event sink: channel → JS callback ──
		let (event_tx, mut event_rx) = mpsc::channel::<(String, Vec<u8>)>(256);
		let cb = on_event.clone();
		spawn_local(async move {
			while let Some((sub_id, bytes)) = event_rx.next().await {
				// Prepend 4-byte LE length so TS can use ArrayBufferReader
				let mut batched = Vec::with_capacity(4 + bytes.len());
				batched.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
				batched.extend_from_slice(&bytes);
				let arr = Uint8Array::new_with_length(batched.len() as u32);
				arr.copy_from(&batched);
				let _ = cb.call2(&JsValue::NULL, &sub_id.into(), &arr.into());
			}
		});

		let engine = Rc::new(NostrEngine::new(transport, storage, event_tx));

		Self { engine }
	}

	/// Dispatch a FlatBuffers MainMessage byte slice to the engine.
	#[wasm_bindgen(js_name = handleMessage)]
	pub fn handle_message(&self, bytes: &[u8]) {
		let engine = self.engine.clone();
		let bytes = bytes.to_vec();
		spawn_local(async move {
			if let Err(e) = engine.handle_message(&bytes).await {
				tracing::warn!("[nipworker-engine] handle_message error: {}", e);
			}
		});
	}

	/// Wake the engine (e.g., after returning from background).
	pub fn wake(&self) {
		info!("[nipworker-engine] wake called");
	}
}
