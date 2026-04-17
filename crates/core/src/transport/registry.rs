//! Connection Registry - Refactored to use the platform-agnostic Transport trait.
//!
//! - Exposes `send_to_relays(relays, frames, ...)`.
//! - Depends on `Arc<dyn Transport>` instead of MessagePort writer closures.
//! - Manages relay URLs and routes incoming messages via the Transport's callbacks.
//! - Handles NIP-42 auth responses from crypto worker.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use wasm_bindgen_futures::spawn_local;

use crate::traits::{RelayTransport, TransportStatus};
use crate::transport::fb_utils::parse_relay_response;
use crate::transport::types::{RelayConfig, RelayError};
use crate::utils::normalize_relay_url;
use serde_json::Value;

pub struct ConnectionRegistry {
	transport: Arc<dyn RelayTransport>,
	config: RelayConfig,
	connected_urls: Arc<RwLock<HashSet<String>>>,
	message_handler: Arc<dyn Fn(&str, &str, &str)>,
	crypto_sender: Arc<dyn Fn(&[u8])>,
}

impl Drop for ConnectionRegistry {
	fn drop(&mut self) {
		tracing::info!("Dropping ConnectionRegistry - all connections will close");
	}
}

impl ConnectionRegistry {
	pub fn new(
		transport: Arc<dyn RelayTransport>,
		message_handler: Arc<dyn Fn(&str, &str, &str)>,
		crypto_sender: Arc<dyn Fn(&[u8])>,
	) -> Self {
		let connected_urls = Arc::new(RwLock::new(HashSet::new()));
		let urls_clone = connected_urls.clone();

		transport.on_status(Box::new(move |status| {
			match status {
				TransportStatus::Connected(url) => {
					urls_clone.write().unwrap().insert(url);
				}
				TransportStatus::Failed(url) | TransportStatus::Closed(url) => {
					urls_clone.write().unwrap().remove(&url);
				}
			}
		}));

		Self {
			transport,
			config: RelayConfig::default(),
			connected_urls,
			message_handler,
			crypto_sender,
		}
	}

	/// Minimal sendToRelays: for each relay, ensure connection and send all frames in order.
	pub fn send_to_relays(
		&self,
		relays: &Vec<String>,
		frames: &Vec<String>,
	) -> Result<(), RelayError> {
		if relays.is_empty() || frames.is_empty() {
			return Ok(());
		}

		for url in relays {
			let normalized_url = normalize_relay_url(&url);

			let should_connect = {
				let connected = self.connected_urls.read().unwrap();
				!connected.contains(&normalized_url)
			};

			if should_connect {
				{
					let mut connected = self.connected_urls.write().unwrap();
					connected.insert(normalized_url.clone());
				}

				let handler = self.message_handler.clone();
				let url_clone = normalized_url.clone();
				self.transport.on_message(
					&normalized_url,
					Box::new(move |msg| {
						let sub_id = parse_relay_response(&msg)
							.and_then(|r| r.sub_id)
							.unwrap_or_default();
						handler(&url_clone, &sub_id, &msg);
					}),
				);

				let transport = self.transport.clone();
				let url_clone = normalized_url.clone();
				let connected = self.connected_urls.clone();
				spawn_local(async move {
					match transport.connect(&url_clone).await {
						Ok(()) => {
							connected.write().unwrap().insert(url_clone);
						}
						Err(e) => {
							tracing::error!("Failed to connect to {}: {:?}", url_clone, e);
						}
					}
				});
			}

			for f in frames {
				if let Err(e) = self.transport.send(&normalized_url, f.clone()) {
					tracing::error!("Send failed for {}: {:?}", normalized_url, e);
				}
			}
		}

		Ok(())
	}

	pub fn close_all(&self, _sub_id: &str) {
		let urls: Vec<String> = self.connected_urls.read().unwrap().iter().cloned().collect();
		for url in urls {
			self.transport.disconnect(&url);
		}
	}

	/// Wake all connections: reset backoffs and trigger immediate reconnection.
	/// Called when app returns from background to foreground.
	pub fn wake_all(&self) {
		tracing::info!("[connections][wake] Waking all connections");
		let urls: Vec<String> = self.connected_urls.read().unwrap().iter().cloned().collect();
		for url in urls {
			let transport = self.transport.clone();
			let url_clone = url.clone();
			spawn_local(async move {
				if let Err(e) = transport.connect(&url_clone).await {
					tracing::warn!("Wake reconnect failed for {}: {:?}", url_clone, e);
				}
			});
		}
	}

	/// Handle signed auth event response from crypto worker
	pub fn handle_auth_response(&self, response_json: &str) {
		tracing::info!("[connections][AUTH] handle_auth_response ENTRY");
		tracing::info!(
			response_len = response_json.len(),
			"[connections][AUTH] handle_auth_response called"
		);
		tracing::debug!(
			response = response_json,
			"[connections][AUTH] Response content"
		);

		let parsed: Value = match serde_json::from_str(response_json) {
			Ok(v) => v,
			Err(e) => {
				tracing::error!("[connections][AUTH] Failed to parse auth response: {}", e);
				return;
			}
		};

		let relay_url = match parsed["relay"].as_str() {
			Some(url) => url,
			None => {
				tracing::error!("[connections][AUTH] Auth response missing relay URL");
				return;
			}
		};

		let signed_event = match parsed["event"].as_str() {
			Some(event) => event,
			None => {
				tracing::error!("[connections][AUTH] Auth response missing signed event");
				return;
			}
		};

		let auth_frame = format!(r#"["AUTH",{}]"#, signed_event);
		if let Err(e) = self.transport.send(relay_url, auth_frame) {
			tracing::error!("[connections][AUTH] Failed to send AUTH frame: {:?}", e);
		}

		tracing::info!("[connections][AUTH] handle_auth_response EXIT");
	}
}

impl Clone for ConnectionRegistry {
	fn clone(&self) -> Self {
		Self {
			transport: self.transport.clone(),
			config: self.config.clone(),
			connected_urls: self.connected_urls.clone(),
			message_handler: self.message_handler.clone(),
			crypto_sender: self.crypto_sender.clone(),
		}
	}
}
