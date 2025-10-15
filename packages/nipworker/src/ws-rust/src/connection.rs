//! Individual Relay Connection Management
//!
//! This module handles individual WebSocket connections to Nostr relays.
//! Each connection manages one relay URL and tracks multiple subscriptions/publishes.

use crate::connection_registry::EventCallback;
use crate::types::{ConnectionStats, ConnectionStatus, RelayConfig, RelayError};
use crate::utils::{extract_first_three, validate_relay_url};

use futures::lock::Mutex;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::RwLock;
use wasm_bindgen_futures::spawn_local;

type StatusWriter = Rc<dyn Fn(&str, &str)>; // (status, url)

/// Individual relay connection managing one WebSocket to one relay
pub struct RelayConnection {
    url: String,
    config: RelayConfig,
    status: Arc<RwLock<ConnectionStatus>>,
    websocket: Arc<RwLock<Option<WebSocket>>>,
    ws_sink: Rc<Mutex<Option<SplitSink<WebSocket, Message>>>>,
    stats: Arc<RwLock<ConnectionStats>>,
    inflight_reqs: Arc<RwLock<i32>>,
    status_writer: Arc<RwLock<Option<StatusWriter>>>,
}

impl RelayConnection {
    pub fn new(url: String, config: RelayConfig) -> Self {
        Self {
            url,
            config,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            websocket: Arc::new(RwLock::new(None)),
            ws_sink: Rc::new(Mutex::new(None)),
            stats: Arc::new(RwLock::new(ConnectionStats::default())),
            inflight_reqs: Arc::new(RwLock::new(0)),
            status_writer: Arc::new(RwLock::new(None)),
        }
    }

    pub fn url(&self) -> &str {
        &self.url
    }
    pub async fn status(&self) -> ConnectionStatus {
        *self.status.read().unwrap()
    }

    pub async fn stats(&self) -> ConnectionStats {
        // If you want truly minimal stats, just return the struct with connected_at set
        self.stats.read().unwrap().clone()
    }

    pub fn set_status_writer(&self, writer: StatusWriter) {
        let mut w = self.status_writer.write().unwrap();
        *w = Some(writer);
    }

    #[inline]
    fn emit_status(&self, status: &str) {
        if let Some(ref f) = *self.status_writer.read().unwrap() {
            f(status, &self.url);
        }
    }

    /// Connect to the relay
    pub async fn connect(&self, cb: EventCallback) -> Result<(), RelayError> {
        // Check if already connected or connecting
        {
            let status = self.status.read().unwrap();
            if matches!(
                *status,
                ConnectionStatus::Connected | ConnectionStatus::Connecting
            ) {
                return Ok(());
            }
        }

        // Set status to connecting
        {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Connecting;
        }

        // Validate URL
        validate_relay_url(&self.url)?;

        // Connect WebSocket
        let websocket = WebSocket::open(&self.url).map_err(|e| {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Failed;
            self.emit_status("failed");
            RelayError::WebSocketError(e.to_string())
        })?;

        // Store WebSocket connection
        {
            let mut ws_guard = self.websocket.write().unwrap();
            *ws_guard = Some(websocket);
        }

        // Set up message handling
        self.setup_message_handling(cb).await.map_err(|e| {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Failed;
            self.emit_status("failed");
            e
        })?;

        // Update status and stats
        {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Connected;
        }
        {
            let mut stats = self.stats.write().unwrap();
            stats.connected_at = Some(js_sys::Date::now() as u64);
        }

        self.emit_status("connected");

        Ok(())
    }

    /// Set up message handling loops
    pub async fn setup_message_handling(&self, on_event: EventCallback) -> Result<(), RelayError> {
        // Take ownership of the websocket
        let websocket = {
            let mut ws_guard = self.websocket.write().unwrap();
            ws_guard.take()
        };
        let Some(websocket) = websocket else {
            return Err(RelayError::ConnectionError(
                "No WebSocket connection".into(),
            ));
        };

        let (ws_sink, mut ws_stream) = websocket.split();

        // Store the sink so send_message can use it directly
        {
            let mut sink_guard = self.ws_sink.lock().await;
            *sink_guard = Some(ws_sink);
        }

        let status_clone = self.status.clone();
        let ws_sink_arc = self.ws_sink.clone();
        let url = self.url.clone();

        let writer_opt = self.status_writer.clone();

        // Single task: read WS → parse JSON → callback
        spawn_local(async move {
            while let Some(message) = ws_stream.next().await {
                match message {
                    Ok(Message::Text(text)) => {
                        if let Some(parts) = extract_first_three(&text) {
                            if let Some(kind_raw) = parts[0] {
                                // Strip enclosing quotes from kind
                                let kind = kind_raw.trim_matches('"');
                                match kind {
                                    "EVENT" => {
                                        let id = parts[1]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        let raw_event_json = parts[2].unwrap_or("{}");
                                        on_event(id, kind, raw_event_json, &url).await;
                                    }
                                    "EOSE" => {
                                        let id = parts[1]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        on_event(id, kind, "", &url).await;
                                    }
                                    "OK" => {
                                        let id = parts[1]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        let state = parts[2].unwrap_or("false");
                                        on_event(id, kind, state, &url).await;
                                    }
                                    "CLOSED" => {
                                        let id = parts[1]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        let reason = parts[2]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        on_event(id, kind, &reason, &url).await;
                                    }
                                    "NOTICE" => {
                                        let id = parts[1]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        let reason = parts[2]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        on_event(id, kind, &reason, &url).await;
                                    }
                                    "AUTH" => {
                                        let id = parts[1]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        let challenge = parts[2]
                                            .map(|s| s.trim_matches('"').to_string())
                                            .unwrap_or_default();
                                        on_event(id, kind, &challenge, &url).await;
                                    }
                                    other => {
                                        tracing::warn!("Unknown relay message kind: {}", other);
                                    }
                                }
                            }
                        } else {
                            tracing::warn!("Malformed message from relay: {}", text);
                        }
                    }
                    Ok(Message::Bytes(_)) => {
                        tracing::warn!(relay = %url, "Unexpected binary message in Nostr");
                    }
                    Err(e) => {
                        tracing::error!(relay = %url, error = %e, "WebSocket error");
                        {
                            let mut status = status_clone.write().unwrap();
                            *status = ConnectionStatus::Failed;
                        }
                        {
                            let mut sink_guard = ws_sink_arc.lock().await;
                            *sink_guard = None;
                        }
                        // report failure on socket error
                        if let Some(f) = &*writer_opt.read().unwrap() {
                            f("failed", &url);
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// Send a raw JSON frame to the relay, updating in-flight REQ count by scanning the frame.
    /// When the count reaches 0 or less, the connection is closed immediately.
    pub async fn send_raw(&self, text: &str) -> Result<(), RelayError> {
        // Check connection status
        {
            let status = self.status.read().unwrap();
            if !status.is_connected() {
                return Err(RelayError::ConnectionClosed);
            }
        }

        // Lock the sink
        let mut sink_guard = self.ws_sink.lock().await;
        let sink = sink_guard.as_mut().ok_or(RelayError::ConnectionClosed)?;

        // Try to send
        if let Err(e) = sink.send(Message::Text(text.to_owned())).await {
            tracing::error!(error = %e, "Failed to send message: marking connection closed");

            // 🔹 Mark connection status immediately
            {
                let mut status_guard = self.status.write().unwrap();
                *status_guard = ConnectionStatus::Failed; // or ConnectionStatus::Closed
            }

            // 🔹 Drop sink so future sends fail fast
            *sink_guard = None;

            self.emit_status("failed");

            return Err(RelayError::ConnectionClosed);
        }

        // Update in-flight count by scanning the frame kind
        let kind = {
            // simple detection: ["REQ" ...] / ["CLOSE" ...]
            let bytes = text.as_bytes();
            let mut i = 0usize;
            while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'[' {
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                if i < bytes.len() && bytes[i] == b'"' {
                    i += 1;
                    let start = i;
                    while i < bytes.len() && bytes[i] != b'"' {
                        i += 1;
                    }
                    if i <= bytes.len() {
                        Some(&text[start..i])
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        };

        if let Some(k) = kind {
            match k {
                "REQ" => {
                    let mut c = self.inflight_reqs.write().unwrap();
                    *c += 1;
                }
                "CLOSE" => {
                    let mut c = self.inflight_reqs.write().unwrap();
                    *c -= 1;
                    if *c <= 0 {
                        // Close immediately when no active REQ left
                        drop(sink_guard); // release sink before awaiting close
                        let _ = self.close().await;
                    }
                }
                _ => {}
            }
        }

        Ok(())
    }

    /// Reconnect to the relay
    pub async fn reconnect(&self, cb: EventCallback) -> Result<(), RelayError> {
        // Close existing connection
        self.close().await?;

        // Update stats
        {
            let mut stats = self.stats.write().unwrap();
            stats.reconnect_attempts += 1;
        }

        // Attempt to reconnect
        self.connect(cb).await
    }

    /// Check if connection is ready for operations
    pub async fn is_ready(&self) -> bool {
        let status = self.status.read().unwrap();
        status.is_connected()
    }

    /// Wait for connection to be ready or return error
    pub async fn wait_for_ready(&self) -> Result<(), RelayError> {
        let status = self.status.read().unwrap();
        match *status {
            ConnectionStatus::Connected => Ok(()),
            ConnectionStatus::Connecting => {
                drop(status);
                // Simple polling approach since we can't use tokio::time in WASM
                for _ in 0..50 {
                    // 5 second timeout with 100ms intervals
                    gloo_timers::future::TimeoutFuture::new(100).await;
                    let current_status = self.status.read().unwrap();
                    match *current_status {
                        ConnectionStatus::Connected => return Ok(()),
                        ConnectionStatus::Failed | ConnectionStatus::Closed => {
                            return Err(RelayError::ConnectionError(
                                "Connection failed".to_string(),
                            ));
                        }
                        _ => continue,
                    }
                }
                Err(RelayError::Timeout)
            }
            ConnectionStatus::Disconnected
            | ConnectionStatus::Failed
            | ConnectionStatus::Closed => Err(RelayError::ConnectionClosed),
        }
    }

    /// Close the connection
    pub async fn close(&self) -> Result<(), RelayError> {
        // Update status
        {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Closed;
        }

        // Close WebSocket
        {
            let mut ws_guard = self.websocket.write().unwrap();
            if let Some(ws) = ws_guard.take() {
                let _ = ws.close(None, None);
            }
        }

        self.emit_status("close");

        Ok(())
    }
}

impl Drop for RelayConnection {
    fn drop(&mut self) {
        // Spawn a task to close the connection since we can't await in Drop
        let status = self.status.clone();
        let websocket = self.websocket.clone();
        let url = self.url.clone();

        spawn_local(async move {
            // Update status
            {
                let mut status = status.write().unwrap();
                *status = ConnectionStatus::Closed;
            }

            // Close WebSocket
            {
                let mut ws_guard = websocket.write().unwrap();
                if let Some(ws) = ws_guard.take() {
                    let _ = ws.close(None, None);
                }
            }
        });
    }
}
