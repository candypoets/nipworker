use futures::channel::mpsc;
use futures::StreamExt;
use js_sys::Uint8Array;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::traits::Signer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;

mod idb_utils;
mod ring_buffer_persist;
mod signer;
mod storage;
mod transport;

use signer::{LocalSigner, ProxySigner};
use storage::NostrDbStorage;
use transport::WebSocketTransport;

/// WASM-facing engine worker that hosts the full NostrEngine in a single thread.
/// Thin wrapper — all orchestration lives in the TypeScript worker.
#[wasm_bindgen]
pub struct NipworkerEngine {
	engine: Rc<NostrEngine>,
	proxy_signer: Rc<RefCell<Option<Arc<ProxySigner>>>>,
	signer_req_tx: mpsc::UnboundedSender<(u64, String, serde_json::Value)>,
}

#[wasm_bindgen]
impl NipworkerEngine {
	/// new(on_event, on_signer_request)
	///
	/// `on_event`      : (subId: string, data: Uint8Array) => void
	/// `on_signer_request` : (id: number, op: string, payload: any) => void
	#[wasm_bindgen(constructor)]
	pub fn new(on_event: js_sys::Function, on_signer_request: js_sys::Function) -> Self {
		console_error_panic_hook::set_once();
		tracing_wasm::set_as_global_default();
		info!("[nipworker-engine] Initializing WASM engine...");

		let transport = Arc::new(WebSocketTransport::new());
		let storage = Arc::new(NostrDbStorage::new(8 * 1024 * 1024));
		let signer: Arc<dyn Signer> = Arc::new(LocalSigner::new());

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

		// ── Signer proxy: channel → JS callback ──
		let (signer_req_tx, mut signer_req_rx) =
			mpsc::unbounded::<(u64, String, serde_json::Value)>();
		let cb = on_signer_request.clone();
		spawn_local(async move {
			while let Some((id, op, payload)) = signer_req_rx.next().await {
				let payload_str =
					serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
				let payload_js = js_sys::JSON::parse(&payload_str)
					.unwrap_or_else(|_| js_sys::Object::new().into());
				let _ = cb.call3(
					&JsValue::NULL,
					&JsValue::from_f64(id as f64),
					&op.into(),
					&payload_js,
				);
			}
		});

		let engine = Rc::new(NostrEngine::new(transport, storage, signer, event_tx));

		Self {
			engine,
			proxy_signer: Rc::new(RefCell::new(None)),
			signer_req_tx,
		}
	}

	/// Switch to a proxy signer (nip07 / nip46) that round-trips to JS.
	#[wasm_bindgen(js_name = setProxySigner)]
	pub fn set_proxy_signer(&self, signer_type: String) {
		info!("[nipworker-engine] set_proxy_signer: {}", signer_type);

		let ps = Arc::new(ProxySigner::new(self.signer_req_tx.clone()));
		*self.proxy_signer.borrow_mut() = Some(Arc::clone(&ps));

		let engine = self.engine.clone();
		spawn_local(async move {
			engine.set_signer(ps).await;
		});
	}

	/// Forward a signer response from JS back to the pending ProxySigner request.
	#[wasm_bindgen(js_name = handleSignerResponse)]
	pub async fn handle_signer_response(&self, id: u64, result: String, error: String) {
		if let Some(ps) = self.proxy_signer.borrow().as_ref() {
			let res = if error.is_empty() {
				Ok(result)
			} else {
				Err(error)
			};
			ps.handle_response(id, res).await;
		}
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
