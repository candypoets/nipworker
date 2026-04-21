use futures::channel::mpsc;
use futures::StreamExt;
use js_sys::{ArrayBuffer, Uint8Array};
use nipworker_core::generated::nostr::fb;
use nipworker_core::service::engine::NostrEngine;
use nipworker_core::traits::Signer;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tracing::info;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::spawn_local;
use web_sys::{MessageEvent, MessagePort};

mod idb_utils;
mod ring_buffer_persist;
mod signer;
mod storage;
mod transport;

use signer::{LocalSigner, ProxySigner};
use storage::NostrDbStorage;
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
	proxy_signer: Rc<RefCell<Option<Arc<ProxySigner>>>>,
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
		tracing_wasm::set_as_global_default();
		info!("[nipworker-engine] Initializing WASM engine...");

		let transport = Arc::new(WebSocketTransport::new());
		let storage = Arc::new(NostrDbStorage::new(8 * 1024 * 1024)); // 8MB default ring buffer
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

		// Set up signer request channel to forward proxy signer requests to JS.
		let (signer_req_tx, mut signer_req_rx) =
			mpsc::unbounded::<(u64, String, serde_json::Value)>();
		let port_for_signer = port.clone();
		spawn_local(async move {
			while let Some((id, op, payload)) = signer_req_rx.next().await {
				let obj = js_sys::Object::new();
				let _ = js_sys::Reflect::set(&obj, &"type".into(), &"signer_request".into());
				let _ = js_sys::Reflect::set(&obj, &"id".into(), &JsValue::from_f64(id as f64));
				let _ = js_sys::Reflect::set(&obj, &"op".into(), &op.into());
				let payload_str =
					serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
				let payload_js = js_sys::JSON::parse(&payload_str)
					.unwrap_or_else(|_| js_sys::Object::new().into());
				let _ = js_sys::Reflect::set(&obj, &"payload".into(), &payload_js);
				let _ = port_for_signer.post_message(&obj);
			}
		});

		let proxy_signer: Rc<RefCell<Option<Arc<ProxySigner>>>> = Rc::new(RefCell::new(None));
		let proxy_signer_for_handler = proxy_signer.clone();

		// Set up onmessage handler
		let closure = Closure::wrap(Box::new(move |event: MessageEvent| {
			// First check for JSON control messages
			if let Ok(obj) = event.data().dyn_into::<js_sys::Object>() {
				if let Ok(ty_val) = js_sys::Reflect::get(&obj, &"type".into()) {
					if let Some(ty) = ty_val.as_string() {
						match ty.as_str() {
							"set_proxy_signer" => {
								if let Ok(signer_type_val) =
									js_sys::Reflect::get(&obj, &"signerType".into())
								{
									if let Some(signer_type) = signer_type_val.as_string() {
										info!(
											"[nipworker-engine] set_proxy_signer: {}",
											signer_type
										);
										let ps =
											Arc::new(ProxySigner::new(signer_req_tx.clone()));
										*proxy_signer_for_handler.borrow_mut() =
											Some(Arc::clone(&ps));
										let engine = engine_for_messages.clone();
										spawn_local(async move {
											engine.set_signer(ps).await;
										});
									}
								}
								return;
							}
							"signer_response" => {
								if let Some(ps) = proxy_signer_for_handler.borrow().as_ref() {
									if let Ok(id_val) =
										js_sys::Reflect::get(&obj, &"id".into())
									{
										if let Some(id) = id_val.as_f64() {
											let id = id as u64;
											let result = if let Ok(result_val) =
												js_sys::Reflect::get(&obj, &"result".into())
											{
												if let Some(result_str) = result_val.as_string() {
													Ok(result_str)
												} else {
													Err("Invalid result type".to_string())
												}
											} else if let Ok(error_val) =
												js_sys::Reflect::get(&obj, &"error".into())
											{
												if let Some(error_str) = error_val.as_string() {
													Err(error_str)
												} else {
													Err("Invalid error type".to_string())
												}
											} else {
												Err("Missing result and error".to_string())
											};
											let ps = Arc::clone(ps);
											spawn_local(async move {
												ps.handle_response(id, result).await;
											});
										}
									}
								}
								return;
							}
							_ => {}
						}
					}
				}

				// Not a control message, try serializedMessage format
				if let Ok(serialized) =
					js_sys::Reflect::get(&obj, &"serializedMessage".into())
				{
					if let Ok(uint8) = serialized.dyn_into::<Uint8Array>() {
						let bytes = uint8.to_vec();
						let engine = engine_for_messages.clone();
						spawn_local(async move {
							if let Err(e) = engine.handle_message(&bytes).await {
								tracing::warn!(
									"[nipworker-engine] handle_message error: {}",
									e
								);
							}
						});
					}
				}
				return;
			}

			// FlatBuffers bytes
			let bytes = if let Ok(data) = event.data().dyn_into::<ArrayBuffer>() {
				let array = Uint8Array::new(&data);
				array.to_vec()
			} else if let Ok(data) = event.data().dyn_into::<Uint8Array>() {
				data.to_vec()
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

		Self {
			engine,
			port,
			signer,
			proxy_signer,
		}
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
