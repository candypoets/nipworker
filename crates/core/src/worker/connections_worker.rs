	use crate::channel::{WorkerChannel, WorkerChannelSender};
	use crate::generated::nostr::fb;
	use crate::spawn::spawn_worker;
	use crate::traits::RelayTransport;
	use crate::transport::connection::RelayConnection;
	use crate::transport::fb_utils::{build_worker_message, serialize_connection_status};
	use crate::transport::types::RelayConfig;
use futures::{SinkExt, StreamExt};
	use std::cell::RefCell;
	use std::collections::HashMap;
	use std::rc::Rc;
	use std::sync::{Arc, RwLock};
	use tracing::{info, warn};

	pub struct ConnectionsWorker {
		transport: Arc<dyn RelayTransport>,
		connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
	}

	impl ConnectionsWorker {
		pub fn new(transport: Arc<dyn RelayTransport>) -> Self {
			Self {
				transport,
				connections: Arc::new(RwLock::new(HashMap::new())),
			}
		}

		pub fn run(
			self,
			mut from_parser: Box<dyn WorkerChannel>,
			to_parser: Box<dyn WorkerChannelSender>,
			mut from_cache: Box<dyn WorkerChannel>,
			mut from_crypto: Box<dyn WorkerChannel>,
			to_crypto: Box<dyn WorkerChannelSender>,
		) {
			// Bridge multiple callback clones into the single WorkerChannelSender
			let (parser_tx, mut parser_rx) = futures::channel::mpsc::unbounded::<Vec<u8>>();
			spawn_worker(async move {
				while let Some(bytes) = parser_rx.next().await {
					if let Err(e) = to_parser.send(&bytes) {
						warn!("[ConnectionsWorker] failed to forward to parser: {}", e);
						break;
					}
				}
			});

		let to_crypto_rc = std::rc::Rc::new(to_crypto);
			let get_or_create_connection = {
				let transport = self.transport.clone();
				let connections = self.connections.clone();
				let parser_tx = parser_tx.clone();
				let to_crypto_rc = to_crypto_rc.clone();
				move |url: &str| {
					{
						let map = connections.read().unwrap();
						if let Some(conn) = map.get(url) {
							return conn.clone();
						}
					}

					let url_string = url.to_string();
					let tx_msg = parser_tx.clone();
					let tx_status = parser_tx.clone();
					let transport = transport.clone();

					let out_writer: Rc<dyn Fn(&str, &str, &str)> =
						Rc::new(move |url: &str, sub_id: &str, msg: &str| {
							let mut fbb = flatbuffers::FlatBufferBuilder::new();
							let wm = build_worker_message(&mut fbb, sub_id, url, msg);
							fbb.finish(wm, None);
							let _ = tx_msg.unbounded_send(fbb.finished_data().to_vec());
						});

					let status_writer: Rc<dyn Fn(&str, &str)> =
						Rc::new(move |status: &str, url: &str| {
							let bytes = serialize_connection_status(url, status, "");
							let _ = tx_status.unbounded_send(bytes);
						});

					let to_crypto_cb: Rc<RefCell<dyn Fn(&[u8])>> = Rc::new(RefCell::new({
						let sender = to_crypto_rc.clone();
						move |bytes: &[u8]| {
							let _ = sender.send(bytes);
						}
					}));

					let conn = RelayConnection::new(
						url_string,
						transport,
						out_writer,
						status_writer,
						to_crypto_cb,
					);

					{
						let mut map = connections.write().unwrap();
						map.insert(url.to_string(), conn.clone());
					}

					conn
				}
			};

			// Loop for messages from parser (e.g. CLOSE, EVENT publish)
			// NOTE: Currently dead code - ParserWorker does not send Raw/NostrEvent directly.
			// The intended flow is Engine → ParserWorker → CacheWorker → ConnectionsWorker.
			// This loop exists for future architecture changes and is tested but not exercised in production.
			let get_conn_parser = get_or_create_connection.clone();
			let connections_parser = self.connections.clone();
			spawn_worker(async move {
				info!("[ConnectionsWorker] parser loop started");
				loop {
					match from_parser.recv().await {
						Ok(bytes) => {
							let wm = match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
								Ok(w) => w,
								Err(_) => {
									warn!(
										"[ConnectionsWorker] Failed to decode WorkerMessage from parser"
									);
									continue;
								}
							};
							let url = wm.url().unwrap_or("");
							match wm.type_() {
								fb::MessageType::Raw => {
									if let Some(raw) = wm.content_as_raw() {
										let text = raw.raw();
										if !text.is_empty() && !url.is_empty() {
											let conn = get_conn_parser(url);
											let _ = conn.send_raw(text);
										}
									}
								}
								fb::MessageType::NostrEvent => {
									if let Some(ev) = wm.content_as_nostr_event() {
										let tags: Vec<serde_json::Value> = ev
											.tags()
											.iter()
											.map(|sv| {
												let arr: Vec<serde_json::Value> = sv
													.items()
													.map(|items| {
														(0..items.len())
															.map(|i| {
																	serde_json::Value::String(
																	items.get(i).to_string(),
																)
															})
															.collect()
													})
													.unwrap_or_default();
												serde_json::Value::Array(arr)
											})
											.collect();
										let event_json = serde_json::json!({
											"id": ev.id(),
											"pubkey": ev.pubkey(),
											"kind": ev.kind(),
											"content": ev.content(),
											"tags": tags,
											"created_at": ev.created_at(),
											"sig": ev.sig(),
										});
										let frame = serde_json::json!(["EVENT", event_json]);
										if let Ok(text) = serde_json::to_string(&frame) {
											if !url.is_empty() {
												let conn = get_conn_parser(url);
												let _ = conn.send_raw(&text);
											}
										}
									}
								}
								fb::MessageType::ConnectionStatus => {
									if let Some(cs) = wm.content_as_connection_status() {
										match cs.status() {
											"CLOSE" => {
												if !url.is_empty() {
															let conn = get_conn_parser(url);
															let _ = conn.close();
															let mut map = connections_parser.write().unwrap();
															map.remove(url);
												}
											}
											_ => {}
										}
									}
								}
								_ => {}
							}
						}
						Err(_) => break,
					}
				}
				info!("[ConnectionsWorker] parser loop exiting");
			});

			// Loop for envelopes from cache (e.g. REQ frames)
			let get_conn_cache = get_or_create_connection.clone();
			spawn_worker(async move {
				info!("[ConnectionsWorker] cache loop started");
				#[derive(serde::Deserialize)]
				struct Envelope {
					relays: Vec<String>,
					frames: Vec<String>,
				}
				loop {
					match from_cache.recv().await {
						Ok(bytes) => {
							let env: Envelope = match serde_json::from_slice(&bytes) {
								Ok(e) => e,
								Err(_) => {
									warn!(
										"[ConnectionsWorker] Failed to parse envelope from cache"
									);
									continue;
								}
							};
							for relay in &env.relays {
								if relay.is_empty() {
									continue;
								}
								let conn = get_conn_cache(relay);
								for frame in &env.frames {
									if let Err(e) = conn.send_raw(frame) {
										warn!(
											"[ConnectionsWorker] send_raw failed for {}: {:?}",
											relay, e
										);
									}
								}
							}
						}
						Err(_) => break,
					}
				}
				info!("[ConnectionsWorker] cache loop exiting");
			});

		// Loop for crypto responses (e.g. NIP-42 AUTH signed events)
		let connections_crypto = self.connections.clone();
		spawn_worker(async move {
			info!("[ConnectionsWorker] crypto loop started");
			loop {
				match from_crypto.recv().await {
					Ok(bytes) => {
						let resp = match flatbuffers::root::<fb::SignerResponse>(&bytes) {
							Ok(r) => r,
							Err(e) => {
								warn!(
									"[ConnectionsWorker] failed to decode SignerResponse from crypto: {}",
									e
								);
								continue;
							}
						};

						let request_id = resp.request_id();
						if request_id < 0x8000_0000_0000_0000 {
							continue;
						}

						if let Some(result_str) = resp.result() {
							if let Ok(parsed) =
								serde_json::from_str::<serde_json::Value>(result_str)
							{
								let relay_url = parsed["relay"].as_str().unwrap_or("");
								let event = parsed["event"].as_str().unwrap_or("");
								if !relay_url.is_empty() && !event.is_empty() {
									let map = connections_crypto.read().unwrap();
									if let Some(conn) = map.get(relay_url) {
										conn.process_signed_auth(event);
									}
								}
							}
						}
					}
					Err(_) => break,
				}
			}
			info!("[ConnectionsWorker] crypto loop exiting");
		});
		}
	}

	#[cfg(all(test, not(target_arch = "wasm32")))]
	mod tests {
		use super::*;
		use crate::channel::TokioWorkerChannel;
		use crate::generated::nostr::fb;
		use crate::traits::{RelayTransport, TransportError, TransportStatus};
		use async_trait::async_trait;
		use std::collections::HashMap;
		use std::sync::{Arc, Mutex, RwLock};
		use tokio::task::LocalSet;

		#[derive(Clone, Debug)]
		enum Call {
			Connect(String),
			Disconnect(String),
			Send(String, String),
		}

		#[derive(Clone)]
		struct MockRelayTransport {
			calls: Arc<Mutex<Vec<Call>>>,
			message_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(String)>>>>,
			status_callbacks: Arc<RwLock<HashMap<String, Box<dyn Fn(TransportStatus)>>>>,
			connect_result: Arc<RwLock<Result<(), TransportError>>>,
			on_connect_callback: Arc<Mutex<Option<Box<dyn Fn(&MockRelayTransport)>>>>,
		}

		impl MockRelayTransport {
			fn new() -> Self {
				Self {
					calls: Arc::new(Mutex::new(Vec::new())),
					message_callbacks: Arc::new(RwLock::new(HashMap::new())),
					status_callbacks: Arc::new(RwLock::new(HashMap::new())),
					connect_result: Arc::new(RwLock::new(Ok(()))),
					on_connect_callback: Arc::new(Mutex::new(None)),
				}
			}

			fn set_connect_result(&self, result: Result<(), TransportError>) {
				*self.connect_result.write().unwrap() = result;
			}

			fn set_on_connect_callback(&self, callback: Box<dyn Fn(&MockRelayTransport)>) {
				*self.on_connect_callback.lock().unwrap() = Some(callback);
			}

			fn calls(&self) -> Vec<Call> {
				self.calls.lock().unwrap().clone()
			}

			fn invoke_message_callback(&self, url: &str, msg: String) {
				let cbs = self.message_callbacks.read().unwrap();
				if let Some(cb) = cbs.get(url) {
					cb(msg);
				}
			}

			fn invoke_status_callback(&self, url: &str, status: TransportStatus) {
				let cbs = self.status_callbacks.read().unwrap();
				if let Some(cb) = cbs.get(url) {
					cb(status);
				}
			}
		}

		#[async_trait(?Send)]
		impl RelayTransport for MockRelayTransport {
			async fn connect(&self, url: &str) -> Result<(), TransportError> {
				self.calls.lock().unwrap().push(Call::Connect(url.to_string()));
				if let Ok(cb_guard) = self.on_connect_callback.lock() {
					if let Some(cb) = cb_guard.as_ref() {
						cb(self);
					}
				}
				self.connect_result.read().unwrap().clone()
			}

			fn disconnect(&self, url: &str) {
				self.calls.lock().unwrap().push(Call::Disconnect(url.to_string()));
			}

			async fn send(&self, url: &str, frame: String) -> Result<(), TransportError> {
				self.calls.lock().unwrap().push(Call::Send(url.to_string(), frame));
				Ok(())
			}

			fn on_message(&self, url: &str, callback: Box<dyn Fn(String)>) {
				self.message_callbacks.write().unwrap().insert(url.to_string(), callback);
			}

			fn on_status(&self, url: &str, callback: Box<dyn Fn(TransportStatus)>) {
				self.status_callbacks.write().unwrap().insert(url.to_string(), callback);
			}
		}

		fn build_raw_worker_message(url: &str, raw: &str) -> Vec<u8> {
			let mut fbb = flatbuffers::FlatBufferBuilder::new();
			let url_off = fbb.create_string(url);
			let raw_off = fbb.create_string(raw);
			let raw_msg = fb::Raw::create(&mut fbb, &fb::RawArgs { raw: Some(raw_off) });
			let wm = fb::WorkerMessage::create(
				&mut fbb,
				&fb::WorkerMessageArgs {
					sub_id: None,
					url: Some(url_off),
					type_: fb::MessageType::Raw,
					content_type: fb::Message::Raw,
					content: Some(raw_msg.as_union_value()),
				},
			);
			fbb.finish(wm, None);
			fbb.finished_data().to_vec()
		}

		fn build_nostr_event_worker_message(url: &str) -> Vec<u8> {
			let mut fbb = flatbuffers::FlatBufferBuilder::new();
			let url_off = fbb.create_string(url);

			let s1 = fbb.create_string("p");
			let s2 = fbb.create_string("pubkey1");
			let tag1_items = fbb.create_vector(&[s1, s2]);
			let tag1 = fb::StringVec::create(&mut fbb, &fb::StringVecArgs { items: Some(tag1_items) });
			let tags = fbb.create_vector(&[tag1]);

			let id_off = fbb.create_string("event_id_123");
			let pubkey_off = fbb.create_string("pubkey_123");
			let content_off = fbb.create_string("hello world");
			let sig_off = fbb.create_string("sig_123");

			let event = fb::NostrEvent::create(
				&mut fbb,
				&fb::NostrEventArgs {
					id: Some(id_off),
					pubkey: Some(pubkey_off),
					kind: 1,
					content: Some(content_off),
					tags: Some(tags),
					created_at: 1234567890,
					sig: Some(sig_off),
				},
			);

			let wm = fb::WorkerMessage::create(
				&mut fbb,
				&fb::WorkerMessageArgs {
					sub_id: None,
					url: Some(url_off),
					type_: fb::MessageType::NostrEvent,
					content_type: fb::Message::NostrEvent,
					content: Some(event.as_union_value()),
				},
			);
			fbb.finish(wm, None);
			fbb.finished_data().to_vec()
		}

		fn build_close_worker_message(url: &str) -> Vec<u8> {
			let mut fbb = flatbuffers::FlatBufferBuilder::new();
			let url_off = fbb.create_string(url);
			let status_off = fbb.create_string("CLOSE");
			let cs = fb::ConnectionStatus::create(
				&mut fbb,
				&fb::ConnectionStatusArgs {
					relay_url: Some(url_off),
					status: Some(status_off),
					message: None,
				},
			);
			let wm = fb::WorkerMessage::create(
				&mut fbb,
				&fb::WorkerMessageArgs {
					sub_id: None,
					url: Some(url_off),
					type_: fb::MessageType::ConnectionStatus,
					content_type: fb::Message::ConnectionStatus,
					content: Some(cs.as_union_value()),
				},
			);
			fbb.finish(wm, None);
			fbb.finished_data().to_vec()
		}

		async fn setup() -> (
			Arc<MockRelayTransport>,
			TokioWorkerChannel,
			TokioWorkerChannel,
			TokioWorkerChannel,
			TokioWorkerChannel,
		) {
			let (parser_test, parser_worker) = TokioWorkerChannel::new_pair();
			let (parser_out_worker, parser_out_test) = TokioWorkerChannel::new_pair();
			let (cache_test, cache_worker) = TokioWorkerChannel::new_pair();
			let (crypto_test, crypto_worker) = TokioWorkerChannel::new_pair();
			let crypto_sender = crypto_worker.clone_sender();

			let transport = Arc::new(MockRelayTransport::new());
			let worker = ConnectionsWorker::new(transport.clone());

			worker.run(
				Box::new(parser_worker),
				parser_out_worker.clone_sender(),
				Box::new(cache_worker),
				Box::new(crypto_worker),
				crypto_sender,
			);

			(transport, parser_test, parser_out_test, cache_test, crypto_test)
		}

		#[tokio::test]
		async fn test_parser_raw_message_sent_to_transport() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, _parser_out_test, _cache_test, _crypto_test) = setup().await;

					let msg = build_raw_worker_message("wss://r", "hello");
					parser_test.send(&msg).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
						"connect was not called"
					);
					assert!(
						calls.iter().any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == "hello")),
						"send was not called with correct frame"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_parser_nostr_event_publishes_json_event() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, _parser_out_test, _cache_test, _crypto_test) = setup().await;

					let msg = build_nostr_event_worker_message("wss://r");
					parser_test.send(&msg).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					let calls = transport.calls();
					let send_call = calls
						.iter()
						.find(|c| matches!(c, Call::Send(url, _) if url == "wss://r"))
						.expect("send was not called");
					if let Call::Send(_, frame) = send_call {
						let parsed: serde_json::Value = serde_json::from_str(frame).unwrap();
						let arr = parsed.as_array().unwrap();
						assert_eq!(arr.len(), 2);
						assert_eq!(arr[0], "EVENT");
						let event = &arr[1];
						assert_eq!(event["id"], "event_id_123");
						assert_eq!(event["pubkey"], "pubkey_123");
						assert_eq!(event["kind"], 1);
						assert_eq!(event["content"], "hello world");
						assert_eq!(event["created_at"], 1234567890);
						assert_eq!(event["sig"], "sig_123");
						assert!(event["tags"].is_array());
					}
				})
				.await;
		}

		#[tokio::test]
		async fn test_parser_close_disconnects() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, _parser_out_test, _cache_test, _crypto_test) = setup().await;

					let msg = build_close_worker_message("wss://r");
					parser_test.send(&msg).await.unwrap();
						tokio::time::sleep(std::time::Duration::from_millis(10)).await;

					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Disconnect(url) if url == "wss://r")),
						"disconnect was not called"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_cache_envelope_forwards_req_frames() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, _parser_test, _parser_out_test, cache_test, _crypto_test) = setup().await;

					let envelope = serde_json::json!({
						"relays": ["wss://r"],
						"frames": [r#"["REQ","s1",{}]"#]
					});
					let bytes = serde_json::to_vec(&envelope).unwrap();
					cache_test.send(&bytes).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
						"connect was not called"
					);
					assert!(
						calls.iter().any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == r#"["REQ","s1",{}]"#)),
						"send was not called with correct frame"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_reconnect_failure_does_not_drop_frames() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, _parser_test, _parser_out_test, cache_test, _crypto_test) = setup().await;
					transport.set_connect_result(Err(TransportError::Other("fail".to_string())));

					let envelope = serde_json::json!({
						"relays": ["wss://r"],
						"frames": [r#"["REQ","s1",{}]"#]
					});
					let bytes = serde_json::to_vec(&envelope).unwrap();
					cache_test.send(&bytes).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r")),
						"connect was not called"
					);
					// With RelayConnection, failed initial connect means the queue drainer
					// never starts, so frames are queued but not sent until reconnect succeeds.
					assert!(
						!calls.iter().any(|c| matches!(c, Call::Send(url, _) if url == "wss://r")),
						"send should not be attempted when initial connect fails"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_transport_message_callback_builds_worker_message() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, mut parser_out_test, _cache_test, _crypto_test) = setup().await;

					// Trigger callback registration by sending any message for the URL
					let trigger = build_raw_worker_message("wss://r", "trigger");
					parser_test.send(&trigger).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
						// Drain synthetic status messages from RelayConnection initial connect
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
						let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"


					// Invoke the stored message callback
					transport.invoke_message_callback("wss://r", r#"["EVENT","sub1",{}]"#.to_string());

					let bytes = parser_out_test.recv().await.unwrap();
					let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
					assert_eq!(wm.sub_id(), Some("sub1"), "sub_id mismatch");
				})
				.await;
		}

		#[tokio::test]
		async fn test_transport_status_callback() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, mut parser_out_test, _cache_test, _crypto_test) = setup().await;

					// Trigger callback registration by sending any message for the URL
					let trigger = build_raw_worker_message("wss://r", "trigger");
					parser_test.send(&trigger).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

						// Drain synthetic status messages from RelayConnection initial connect
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
						let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"


					// Invoke the stored status callback
					transport.invoke_status_callback(
						"wss://r",
						TransportStatus::Connected { url: "wss://r".to_string() },
					);

					let bytes = parser_out_test.recv().await.unwrap();
					let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
					assert_eq!(
						wm.type_(),
						fb::MessageType::ConnectionStatus,
						"expected ConnectionStatus message"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_disconnect_during_active_subscription() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, mut parser_out_test, _cache_test, _crypto_test) = setup().await;

					// Create active connection/registration for URL "wss://r1"
					let trigger = build_raw_worker_message("wss://r1", "trigger");
					parser_test.send(&trigger).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

						// Drain synthetic status messages from RelayConnection initial connect
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
						let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"


					// Verify callbacks are set up by invoking them
					transport.invoke_message_callback("wss://r1", r#"["EVENT","sub1",{}]"#.to_string());
					let bytes = parser_out_test.recv().await.unwrap();
					let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();
					assert_eq!(wm.sub_id(), Some("sub1"), "callback should work before disconnect");

					// Send ConnectionStatus with status="CLOSE" to trigger disconnect
					let close_msg = build_close_worker_message("wss://r1");
					parser_test.send(&close_msg).await.unwrap();
					tokio::task::yield_now().await;

					// Verify transport.disconnect("wss://r1") was called
					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Disconnect(url) if url == "wss://r1")),
						"disconnect was not called for wss://r1"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_reconnect_resumes_sending() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, _parser_out_test, cache_test, _crypto_test) = setup().await;

					// Connect and register URL "wss://r1" via cache envelope (which calls connect)
					let envelope = serde_json::json!({
						"relays": ["wss://r1"],
						"frames": [r#"["REQ","s1",{}]"#]
					});
					let bytes = serde_json::to_vec(&envelope).unwrap();
					cache_test.send(&bytes).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					// Verify connect was called
					let calls_before = transport.calls();
					assert!(
						calls_before.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r1")),
						"connect was not called initially"
					);

					// Disconnect it
					let close_msg = build_close_worker_message("wss://r1");
					parser_test.send(&close_msg).await.unwrap();
					tokio::task::yield_now().await;

					// Clear calls to check for new ones
					transport.calls.lock().unwrap().clear();

					// Re-register by sending via cache again (get_or_create returns existing connection)
					let envelope2 = serde_json::json!({
						"relays": ["wss://r1"],
						"frames": [r#"["REQ","s2",{}]"#]
					});
					let bytes2 = serde_json::to_vec(&envelope2).unwrap();
					cache_test.send(&bytes2).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					// Verify new frames can be sent after reconnect
					// Note: connect won't be called again because RelayConnection is reused,
					// but send should still work
					let calls_after = transport.calls();
					assert!(
						calls_after.iter().any(|c| matches!(c, Call::Send(url, frame) if url == "wss://r1" && frame == r#"["REQ","s2",{}]"#)),
						"send was not called with new frame after reconnect"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_multiple_relays_one_fails() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, _parser_test, _parser_out_test, cache_test, _crypto_test) = setup().await;

					// Make all connects fail (MockRelayTransport uses a single shared result)
					transport.set_connect_result(Err(TransportError::Other("fail".to_string())));

					// Send an envelope with 3 relays
					let envelope = serde_json::json!({
						"relays": ["wss://r1", "wss://r2", "wss://r3"],
						"frames": [r#"["REQ","s1",{}]"#]
					});
					let bytes = serde_json::to_vec(&envelope).unwrap();
					cache_test.send(&bytes).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

					// Verify all 3 relays attempted connect
					let calls = transport.calls();
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r1")),
						"r1 connect was not attempted"
					);
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r2")),
						"r2 connect was not attempted"
					);
					assert!(
						calls.iter().any(|c| matches!(c, Call::Connect(url) if url == "wss://r3")),
						"r3 connect was not attempted"
					);

					// With RelayConnection, failed initial connect means queue drainer never starts,
					// so frames are queued but not sent. No Send calls are expected here.
					assert!(
						!calls.iter().any(|c| matches!(c, Call::Send(_, _))),
						"no sends should occur when all initial connects fail"
					);
				})
				.await;
		}

		#[tokio::test]
		async fn test_transport_error_callback_propagation() {
			let local = LocalSet::new();
			local
				.run_until(async {
					let (transport, parser_test, mut parser_out_test, _cache_test, _crypto_test) = setup().await;

					// Register a URL with on_status callback by sending any message
					let trigger = build_raw_worker_message("wss://r1", "trigger");
					parser_test.send(&trigger).await.unwrap();
					tokio::task::yield_now().await;
					tokio::task::yield_now().await;

						// Drain synthetic status messages from RelayConnection initial connect
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (new)
						let _ = parser_out_test.recv().await.unwrap(); // "connecting" (connect)
						let _ = parser_out_test.recv().await.unwrap(); // "connected" or "failed"


					// Invoke the callback with TransportStatus::Failed
					transport.invoke_status_callback(
						"wss://r1",
						TransportStatus::Failed { url: "wss://r1".to_string() },
					);

					// Verify the callback captures the failed status
					let bytes = parser_out_test.recv().await.unwrap();
					let wm = flatbuffers::root::<fb::WorkerMessage>(&bytes).unwrap();

					// The status callback should serialize and send ConnectionStatus bytes
					assert_eq!(
						wm.type_(),
						fb::MessageType::ConnectionStatus,
						"expected ConnectionStatus message for failed status"
					);

					// Verify it's a ConnectionStatus with "failed" status
					if let Some(cs) = wm.content_as_connection_status() {
						assert_eq!(cs.relay_url(), "wss://r1", "relay_url mismatch");
						assert_eq!(cs.status(), "failed", "status should be 'failed'");
					} else {
						panic!("Expected ConnectionStatus content");
					}
				})
				.await;
		}

	#[tokio::test]
	async fn test_auth_event_full_flow() {
		let local = LocalSet::new();
		local
			.run_until(async {
				let (transport, _parser_test, _parser_out_test, cache_test, mut crypto_test) = setup().await;

				// Send cache envelope with REQ frame to establish connection
				let envelope = serde_json::json!({
					"relays": ["wss://r"],
					"frames": [r#"["REQ","s1",{}]"#]
				});
				let bytes = serde_json::to_vec(&envelope).unwrap();
				cache_test.send(&bytes).await.unwrap();

				// Let workers run and connection establish
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Now inject AUTH challenge from relay (after on_message is registered)
				transport.invoke_message_callback(
					"wss://r",
					r#"["AUTH","challenge123"]"#.to_string(),
				);
				tokio::task::yield_now().await;

				// Read SignerRequest from crypto channel
				let crypto_bytes = crypto_test.recv().await.expect("expected crypto request");
				let req = flatbuffers::root::<fb::SignerRequest>(&crypto_bytes).unwrap();
				assert_eq!(req.op(), fb::SignerOp::AuthEvent, "expected AuthEvent op");
				let payload = req.payload().expect("expected payload");
				let parsed: serde_json::Value = serde_json::from_str(payload).unwrap();
				assert_eq!(parsed["challenge"], "challenge123", "challenge mismatch");
				assert_eq!(parsed["relay"], "wss://r", "relay mismatch");
				let request_id = req.request_id();

				// Build SignerResponse with signed event
				let result_json = serde_json::json!({
					"event": r#"{"id":"abc","pubkey":"pk","created_at":123,"kind":22242,"tags":[["challenge","challenge123"],["relay","wss://r"]],"content":"","sig":"sig"}"#,
					"relay": "wss://r"
				})
				.to_string();

				let mut fbb = flatbuffers::FlatBufferBuilder::new();
				let result_off = fbb.create_string(&result_json);
				let resp = fb::SignerResponse::create(
					&mut fbb,
					&fb::SignerResponseArgs {
						request_id,
						result: Some(result_off),
						error: None,
					},
				);
				fbb.finish(resp, None);
				crypto_test.send(fbb.finished_data()).await.unwrap();

				// Let workers process the signed auth response
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify AUTH frame was sent to relay
				let calls = transport.calls();
				let auth_sent = calls.iter().any(|c| {
					matches!(c, Call::Send(url, frame) if url == "wss://r" && frame.starts_with(r#"["AUTH",{"#))
				});
				assert!(auth_sent, "expected AUTH frame to be sent to relay");

				// Inject OK response from relay
				transport.invoke_message_callback(
					"wss://r",
					r#"["OK","auth-id","true"]"#.to_string(),
				);

				// Let auth handling run
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;
				tokio::task::yield_now().await;

				// Verify original REQ frame was sent
				let req_sent = calls.iter().any(|c| {
					matches!(c, Call::Send(url, frame) if url == "wss://r" && frame == r#"["REQ","s1",{}]"#)
				});
				assert!(req_sent, "expected original REQ frame to be sent");
			})
			.await;
	}
}
