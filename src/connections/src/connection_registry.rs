//! Connection Registry - Minimal: sendToRelays and raw incoming forwarding
//!
//! - No subscribe/publish APIs here.
//! - Exposes `send_to_relays(relays, frames, ...)`.
//! - `process_incoming_message` reconstructs raw JSON and forwards it with (url, subId) so
//!   the caller can hash subId to the correct outRing (TS parity).
//! - Handles NIP-42 auth responses from crypto worker.

use wasm_bindgen_futures::spawn_local;

use crate::connection::RelayConnection;
use crate::types::{RelayConfig, RelayError};
use crate::utils::normalize_relay_url;
use serde_json::Value;
use shared::Port;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

/// Writer invoked with resolved target:
/// (url, sub_id, raw_text)
type OutWriter = Rc<dyn Fn(&str, &str, &str)>;

/// Writer for simple status lines: (status, url), where status ∈ {"connected","failed","close"}
type StatusWriter = Rc<dyn Fn(&str, &str)>;

pub struct ConnectionRegistry {
    connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
    config: RelayConfig,
    out_writer: OutWriter,
    status_writer: StatusWriter,
    to_crypto: Rc<RefCell<Port>>,
}

impl Drop for ConnectionRegistry {
    fn drop(&mut self) {
        tracing::info!("Dropping ConnectionRegistry - all connections will close");
    }
}

impl ConnectionRegistry {
    pub fn new(
        out_writer: OutWriter,
        status_writer: StatusWriter,
        to_crypto: Rc<RefCell<Port>>,
    ) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            config: RelayConfig::default(),
            out_writer,
            status_writer,
            to_crypto,
        }
    }

    /// Get a connection by URL
    pub fn get_connection(&self, url: &str) -> Option<Arc<RelayConnection>> {
        let normalized = normalize_relay_url(url);
        self.connections.read().unwrap().get(&normalized).cloned()
    }

    /// Handle signed auth event response from crypto worker
    pub fn handle_auth_response(&self, response_json: &str) {
        tracing::info!("[connections][AUTH] handle_auth_response ENTRY");
        tracing::info!(response_len = response_json.len(), "[connections][AUTH] handle_auth_response called");
        tracing::debug!(response = response_json, "[connections][AUTH] Response content");
        
        // Parse response to get relay URL and signed event
        tracing::info!("[connections][AUTH] Parsing JSON response");
        let parsed: Value = match serde_json::from_str(response_json) {
            Ok(v) => {
                tracing::info!("[connections][AUTH] JSON parsed successfully");
                v
            }
            Err(e) => {
                tracing::error!("[connections][AUTH] Failed to parse auth response: {}", e);
                return;
            }
        };

        tracing::info!("[connections][AUTH] Extracting relay URL");
        let relay_url = match parsed["relay"].as_str() {
            Some(url) => {
                tracing::info!(relay = url, "[connections][AUTH] Found relay URL");
                url
            }
            None => {
                tracing::error!("[connections][AUTH] Auth response missing relay URL");
                return;
            }
        };

        tracing::info!("[connections][AUTH] Extracting signed event");
        let signed_event = match parsed["event"].as_str() {
            Some(event) => {
                tracing::info!(event_len = event.len(), "[connections][AUTH] Found signed event");
                event
            }
            None => {
                tracing::error!("[connections][AUTH] Auth response missing signed event");
                return;
            }
        };

        tracing::info!(relay = relay_url, event_len = signed_event.len(), "[connections][AUTH] Dispatching to connection");

        // Dispatch to the correct connection
        tracing::info!("[connections][AUTH] Looking up connection in registry");
        if let Some(conn) = self.get_connection(relay_url) {
            tracing::info!(relay = relay_url, "[connections][AUTH] Found connection, dispatching signed auth");
            conn.process_signed_auth(signed_event);
            tracing::info!("[connections][AUTH] process_signed_auth completed");
        } else {
            tracing::warn!(relay = relay_url, "[connections][AUTH] No connection found for relay");
        }
        tracing::info!("[connections][AUTH] handle_auth_response EXIT");
    }

    /// Minimal sendToRelays: for each relay, ensure connection and send all frames in order.
    /// No cooldown or retry features. Errors are logged and ignored.
    pub fn send_to_relays(
        &self,
        relays: &Vec<String>,
        frames: &Vec<String>,
    ) -> Result<(), RelayError> {
        if relays.is_empty() || frames.is_empty() {
            return Ok(());
        }

        let mut connections = self.connections.write().unwrap(); // Prevent races

        for url in relays {
            let normalized_url = normalize_relay_url(&url);

            let conn = if let Some(existing) = connections.get(&normalized_url) {
                existing.clone()
            } else {
                let new_conn = RelayConnection::new(
                    normalized_url.clone(),
                    self.config.clone(),
                    self.out_writer.clone(),
                    self.status_writer.clone(),
                    self.to_crypto.clone(),
                );
                connections.insert(normalized_url.clone(), new_conn.clone());
                new_conn
            };

            let strong = Arc::strong_count(&conn);
            let conn_id = Arc::as_ptr(&conn) as usize;
            let keys: Vec<String> = connections.keys().cloned().collect();
            // tracing::info!(%normalized_url, strong, conn_id, ?keys, "Registry: conn strong_count and keys");

            for f in frames {
                if let Err(e) = conn.send_raw(f) {
                    tracing::error!("Send failed for {}: {:?}", normalized_url, e);
                }
            }
        }

        Ok(())
    }
    pub fn close_all(&self, sub_id: &str) {
        let conns: Vec<Arc<RelayConnection>> =
            self.connections.read().unwrap().values().cloned().collect();
        for c in conns {
            let sub = sub_id.to_string();
            spawn_local(async move {
                // No reconnect attempts inside this call.
                let _ = c.close_sub(&sub).await;
            });
        }
    }
}

impl Clone for ConnectionRegistry {
    fn clone(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            config: self.config.clone(),
            out_writer: self.out_writer.clone(),
            status_writer: self.status_writer.clone(),
            to_crypto: self.to_crypto.clone(),
        }
    }
}
