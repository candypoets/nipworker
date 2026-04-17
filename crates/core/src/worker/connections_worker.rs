use crate::channel::{WorkerChannel, WorkerChannelSender};
use crate::generated::nostr::fb;
use crate::spawn::spawn_worker;
use crate::traits::{RelayTransport, TransportStatus};
use crate::transport::fb_utils::{build_worker_message, serialize_connection_status};
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

pub struct ConnectionsWorker {
	transport: Arc<dyn RelayTransport>,
}

impl ConnectionsWorker {
	pub fn new(transport: Arc<dyn RelayTransport>) -> Self {
		Self { transport }
	}

	pub fn run(
		self,
		mut from_parser: Box<dyn WorkerChannel>,
		to_parser: Box<dyn WorkerChannelSender>,
		mut from_cache: Box<dyn WorkerChannel>,
	) {
		// Bridge multiple callback clones into the single WorkerChannelSender
		let (parser_tx, mut parser_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
		spawn_worker(async move {
			while let Some(bytes) = parser_rx.recv().await {
				if let Err(e) = to_parser.send(&bytes) {
					warn!("[ConnectionsWorker] failed to forward to parser: {}", e);
					break;
				}
			}
		});

		let registered_urls: Arc<RwLock<HashSet<String>>> = Arc::new(RwLock::new(HashSet::new()));

		let ensure_registered = {
			let transport = self.transport.clone();
			let registered = registered_urls.clone();
			let parser_tx = parser_tx.clone();
			move |url: &str| {
				let mut set = registered.write().unwrap();
				if set.insert(url.to_string()) {
					let url_msg = url.to_string();
					let url_msg_clone = url_msg.clone();
					let tx_msg = parser_tx.clone();
					transport.on_message(
						&url_msg,
						Box::new(move |msg: String| {
							let sub_id = crate::transport::fb_utils::parse_relay_response(&msg)
								.and_then(|r| r.sub_id)
								.unwrap_or_default();
							let mut fbb = flatbuffers::FlatBufferBuilder::new();
							let wm = build_worker_message(&mut fbb, &sub_id, &url_msg_clone, &msg);
							fbb.finish(wm, None);
							let _ = tx_msg.send(fbb.finished_data().to_vec());
						}),
					);

					let url_status = url.to_string();
					let url_status_clone = url_status.clone();
					let tx_status = parser_tx.clone();
					transport.on_status(
						&url_status,
						Box::new(move |status: TransportStatus| {
							let (_, status_str) = match status {
								TransportStatus::Connected { url } => (url, "connected"),
								TransportStatus::Failed { url } => (url, "failed"),
								TransportStatus::Closed { url } => (url, "closed"),
							};
							let bytes = serialize_connection_status(&url_status_clone, status_str, "");
							let _ = tx_status.send(bytes);
						}),
					);
				}
			}
		};

		// Loop for messages from parser (e.g. CLOSE, EVENT publish)
		let transport_parser = self.transport.clone();
		let ensure_registered_parser = ensure_registered.clone();
		spawn_worker(async move {
			info!("[ConnectionsWorker] parser loop started");
			loop {
				match from_parser.recv().await {
					Ok(bytes) => {
						let wm = match flatbuffers::root::<fb::WorkerMessage>(&bytes) {
							Ok(w) => w,
							Err(_) => {
								warn!("[ConnectionsWorker] Failed to decode WorkerMessage from parser");
								continue;
							}
						};
						let url = wm.url().unwrap_or("");
						if !url.is_empty() {
							ensure_registered_parser(url);
						}
						match wm.type_() {
							fb::MessageType::Raw => {
								if let Some(raw) = wm.content_as_raw() {
									let text = raw.raw();
									if !text.is_empty() && !url.is_empty() {
										let _ = transport_parser.send(url, text.to_string());
									}
								}
							}
							fb::MessageType::NostrEvent => {
								if let Some(ev) = wm.content_as_nostr_event() {
									let tags: Vec<serde_json::Value> = ev.tags().iter().map(|sv| {
										let arr: Vec<serde_json::Value> = sv.items().map(|items| {
											(0..items.len())
												.map(|i| serde_json::Value::String(items.get(i).to_string()))
												.collect()
										}).unwrap_or_default();
										serde_json::Value::Array(arr)
									}).collect();
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
										let _ = transport_parser.send(url, text);
									}
								}
							}
							fb::MessageType::ConnectionStatus => {
								if let Some(cs) = wm.content_as_connection_status() {
									match cs.status() {
										"CLOSE" => transport_parser.disconnect(url),
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
		let transport_cache = self.transport.clone();
		let ensure_registered_cache = ensure_registered.clone();
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
								warn!("[ConnectionsWorker] Failed to parse envelope from cache");
								continue;
							}
						};
						for relay in &env.relays {
							if !relay.is_empty() {
								ensure_registered_cache(relay);
							}
							let t = transport_cache.clone();
							let r = relay.clone();
							// Attempt connect, but proceed with send even if it fails
							// because an existing live socket may already be available.
							if let Err(e) = t.connect(&r).await {
								warn!("[ConnectionsWorker] connect failed for {} (may already be connected): {:?}", r, e);
							}
							for frame in &env.frames {
								if let Err(e) = t.send(&r, frame.clone()) {
									warn!("[ConnectionsWorker] send failed for {}: {:?}", r, e);
								}
							}
						}
					}
					Err(_) => break,
				}
			}
			info!("[ConnectionsWorker] cache loop exiting");
		});
	}
}
