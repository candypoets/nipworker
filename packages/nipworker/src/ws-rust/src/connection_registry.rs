//! Connection Registry - Minimal: sendToRelays and raw incoming forwarding
//!
//! - No subscribe/publish APIs here.
//! - Exposes `send_to_relays(relays, frames, ...)`.
//! - `process_incoming_message` reconstructs raw JSON and forwards it with (url, subId) so
//!   the caller can hash subId to the correct outRing (TS parity).

use crate::connection::RelayConnection;
use crate::types::{ConnectionStatus, RelayConfig, RelayError};
use crate::utils::{extract_first_three, normalize_relay_url};
use futures::future::LocalBoxFuture;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, RwLock};

/// Incoming event callback used by RelayConnection:
/// (sub_id, kind, message, url)
pub type EventCallback = Rc<dyn Fn(String, &str, &str, &str) -> LocalBoxFuture<'static, ()>>;

/// Writer invoked with resolved target:
/// (url, sub_id, raw_text)
type OutWriter = Rc<dyn Fn(&str, &str, &str)>;

/// Writer for simple status lines: (status, url), where status ∈ {"connected","failed","close"}
type StatusWriter = Rc<dyn Fn(&str, &str)>;

pub struct ConnectionRegistry {
    connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
    config: RelayConfig,
    out_writer: Option<OutWriter>,
    status_writer: Option<StatusWriter>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::with_config(RelayConfig::default())
    }

    pub fn with_config(config: RelayConfig) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            config,
            out_writer: None,
            status_writer: None,
        }
    }

    pub fn set_out_writer(&mut self, writer: OutWriter) {
        self.out_writer = Some(writer);
    }

    pub fn set_status_writer(&mut self, writer: StatusWriter) {
        self.status_writer = Some(writer);
    }

    async fn process_incoming_message(
        &self,
        sub_id: String,
        kind: &str,
        message: &str,
        relay_url: &str,
    ) {
        // Reconstruct raw JSON line
        let raw = match kind {
            "EVENT" => format!(r#"["EVENT","{}",{}]"#, sub_id, message),
            "EOSE" => format!(r#"["EOSE","{}"]"#, sub_id),
            "OK" => format!(r#"["OK","{}",{}]"#, sub_id, message),
            "CLOSED" => {
                let esc = message.replace('\\', "\\\\").replace('"', "\\\"");
                format!(r#"["CLOSED","{}","{}"]"#, sub_id, esc)
            }
            "NOTICE" => {
                let esc = message.replace('\\', "\\\\").replace('"', "\\\"");
                format!(r#"["NOTICE","{}"]"#, esc)
            }
            "AUTH" => {
                let esc = message.replace('\\', "\\\\").replace('"', "\\\"");
                format!(r#"["AUTH","{}"]"#, esc)
            }
            other => {
                let esc = other.replace('\\', "\\\\").replace('"', "\\\"");
                format!(r#"["NOTICE","unknown kind: {}"]"#, esc)
            }
        };

        if let Some(ref writer) = self.out_writer {
            if !sub_id.is_empty() {
                writer(relay_url, &sub_id, &raw);
            }
            // If no sub_id, behave like TS: drop it (no ring mapping possible)
        }
    }

    pub async fn ensure_connection(&self, url: &str) -> Result<Arc<RelayConnection>, RelayError> {
        if let Some(conn) = self.get_connection(url).await {
            if let Ok(()) = conn.wait_for_ready().await {
                return Ok(conn);
            }
            let registry = Arc::new(self.clone());
            let cb: EventCallback = Rc::new(move |sub_id, kind, event, url| {
                let reg = registry.clone();
                let kind_owned = kind.to_owned();
                let event_owned = event.to_owned();
                let url_owned = url.to_owned();
                Box::pin(async move {
                    reg.process_incoming_message(sub_id, &kind_owned, &event_owned, &url_owned)
                        .await;
                })
            });
            conn.reconnect(cb).await?;
            // no registry-level status emission; connection.rs does it
            return Ok(conn);
        }

        let connection = Arc::new(RelayConnection::new(url.to_string(), self.config.clone()));

        {
            let mut connections = self.connections.write().unwrap();
            connections.insert(url.to_string(), connection.clone());
        }
        let registry = Arc::new(self.clone());
        let cb: EventCallback = Rc::new(move |sub_id, kind, event, url| {
            let reg = registry.clone();
            let kind_owned = kind.to_owned();
            let event_owned = event.to_owned();
            let url_owned = url.to_owned();
            Box::pin(async move {
                reg.process_incoming_message(sub_id, &kind_owned, &event_owned, &url_owned)
                    .await;
            })
        });
        if let Err(e) = connection.connect(cb).await {
            return Err(e);
        }
        Ok(connection)
    }

    async fn get_connection(&self, url: &str) -> Option<Arc<RelayConnection>> {
        let connections = self.connections.read().unwrap();
        connections.get(url).cloned()
    }

    /// Minimal sendToRelays: for each relay, ensure connection and send all frames in order.
    /// No cooldown or retry features. Errors are logged and ignored.
    pub async fn send_to_relays(
        &self,
        relays: Vec<String>,
        frames: Vec<String>,
        max_successes: usize,
        max_concurrency: usize,
    ) {
        use futures::stream::{self, StreamExt};

        if relays.is_empty() || frames.is_empty() {
            return;
        }

        // Build one job per relay
        let this = self.clone();
        let jobs = relays.into_iter().map(move |url| {
            let this = this.clone();
            let frames = frames.clone();
            async move {
                // Ensure connection and send frames in-order for this relay
                if let Ok(conn) = this.ensure_connection(&url).await {
                    for f in &frames {
                        // Parse kind and sub_id from the outgoing frame
                        let (is_req, sub_id) = if let Some(parts) = extract_first_three(f) {
                            if let Some(kind_raw) = parts[0] {
                                let kind = kind_raw.trim_matches('"');
                                if kind == "REQ" {
                                    let sid = parts[1].map(|s| s.trim_matches('"').to_string());
                                    (true, sid)
                                } else {
                                    (false, None)
                                }
                            } else {
                                (false, None)
                            }
                        } else {
                            (false, None)
                        };

                        // Try to send the frame
                        if let Err(e) = conn.send_raw(f).await {
                            tracing::warn!("send frame failed {}: {}", url, e);

                            // Emit synthetic OK "<sub_id>","FAILED" for REQ
                            if let (true, Some(sub_id)) = (is_req, sub_id.clone()) {
                                // JSON string payload → include quotes
                                this.process_incoming_message(sub_id, "OK", "FAILED", &url)
                                    .await;
                            }

                            // Optional: also inform connection-level status UI
                            if let Some(ref w) = this.status_writer {
                                w("failed", &url);
                            }

                            break;
                        } else {
                            // On success, emit synthetic OK "<sub_id>","SUBSCRIBED" for REQ
                            if let (true, Some(sub_id)) = (is_req, sub_id) {
                                this.process_incoming_message(sub_id, "OK", "SUBSCRIBED", &url)
                                    .await;
                            }
                        }
                    }
                    Ok::<(), ()>(())
                } else {
                    Err::<(), ()>(())
                }
            }
        });

        // Run up to max_concurrency relays in parallel
        let mut successes = 0usize;
        stream::iter(jobs)
            .buffer_unordered(max_concurrency.max(1))
            .for_each(|res| {
                if res.is_ok() {
                    successes += 1;
                }
                futures::future::ready(())
            })
            .await;

        // Optional: if you want to stop early at max_successes like TS,
        // you can track successes and short-circuit by using a shared flag,
        // but the above already unblocks other relays so it addresses the hang.
    }

    pub async fn disconnect(&self, url: &str) -> Result<(), RelayError> {
        let normalized_url = normalize_relay_url(url);
        let connection = {
            let mut connections = self.connections.write().unwrap();
            connections.remove(&normalized_url)
        };
        if let Some(connection) = connection {
            connection.close().await?;
        }
        Ok(())
    }

    pub async fn disconnect_all(&self) -> Result<(), RelayError> {
        let connections = {
            let mut connections_guard = self.connections.write().unwrap();
            connections_guard
                .drain()
                .map(|(_, c)| c)
                .collect::<Vec<_>>()
        };
        for c in connections {
            let _ = c.close().await;
        }
        Ok(())
    }
}

impl Clone for ConnectionRegistry {
    fn clone(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            config: self.config.clone(),
            out_writer: self.out_writer.clone(),
            status_writer: self.status_writer.clone(),
        }
    }
}
