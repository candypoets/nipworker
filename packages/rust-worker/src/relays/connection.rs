//! Individual Relay Connection Management
//!
//! This module handles individual WebSocket connections to Nostr relays.
//! Each connection manages one relay URL and tracks multiple subscriptions/publishes.

use crate::relays::connection_registry::EventCallback;
use crate::relays::types::{
    ClientMessage, ConnectionStats, ConnectionStatus, RelayConfig, RelayError, RelayMessage,
    RelayResponse,
};
use crate::utils::json::extract_first_three;
use futures::channel::mpsc;
use futures::future::{AbortHandle, Abortable};
use futures::lock::Mutex;
use futures::stream::SplitSink;
use futures::{SinkExt, StreamExt};
use gloo_net::websocket::{futures::WebSocket, Message};
use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::RwLock;
use wasm_bindgen_futures::spawn_local;

/// Individual relay connection managing one WebSocket to one relay
pub struct RelayConnection {
    /// Relay URL
    url: String,
    /// Connection configuration
    config: RelayConfig,
    /// Current connection status
    status: Arc<RwLock<ConnectionStatus>>,
    /// WebSocket connection (when connected)
    websocket: Arc<RwLock<Option<WebSocket>>>,
    /// WebSocket sink (when connected)
    ws_sink: Rc<Mutex<Option<SplitSink<WebSocket, Message>>>>,
    /// Outgoing message sender
    outgoing_tx: mpsc::UnboundedSender<ClientMessage>,
    /// Outgoing message receiver for internal use
    outgoing_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<ClientMessage>>>>,
    /// Incoming message receiver for external consumers
    incoming_rx: Arc<RwLock<Option<mpsc::UnboundedReceiver<RelayResponse>>>>,
    /// Connection statistics
    stats: Arc<RwLock<ConnectionStats>>,
    /// Active subscriptions (subscription_id -> filters count)
    active_subscriptions: Arc<RwLock<HashMap<String, usize>>>,
    /// Active publishes (event_id -> publish_id)
    active_publishes: Arc<RwLock<HashMap<String, String>>>,
    /// Connection abort handle
    connection_abort: Arc<RwLock<Option<AbortHandle>>>,
    /// Last activity timestamp
    last_activity: Arc<RwLock<instant::Instant>>,
}

impl RelayConnection {
    /// Create a new relay connection
    pub fn new(url: String, config: RelayConfig) -> Self {
        let (outgoing_tx, outgoing_rx) = mpsc::unbounded();

        Self {
            url,
            config,
            status: Arc::new(RwLock::new(ConnectionStatus::Disconnected)),
            websocket: Arc::new(RwLock::new(None)),
            ws_sink: Rc::new(Mutex::new(None)),
            outgoing_tx,
            outgoing_rx: Arc::new(RwLock::new(Some(outgoing_rx))),
            incoming_rx: Arc::new(RwLock::new(None)),
            stats: Arc::new(RwLock::new(ConnectionStats::default())),
            active_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            active_publishes: Arc::new(RwLock::new(HashMap::new())),
            connection_abort: Arc::new(RwLock::new(None)),
            last_activity: Arc::new(RwLock::new(instant::Instant::now())),
        }
    }

    /// Get the relay URL
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Get current connection status
    pub async fn status(&self) -> ConnectionStatus {
        *self.status.read().unwrap()
    }

    /// Get connection statistics
    pub async fn stats(&self) -> ConnectionStats {
        let mut stats = self.stats.read().unwrap().clone();
        stats.active_subscriptions = self.active_subscriptions.read().unwrap().len();
        stats.active_publishes = self.active_publishes.read().unwrap().len();
        stats
    }

    /// Get the number of active operations (subscriptions + publishes)
    pub async fn operation_count(&self) -> usize {
        let subs = self.active_subscriptions.read().unwrap().len();
        let pubs = self.active_publishes.read().unwrap().len();
        subs + pubs
    }

    /// Check if connection has any active operations
    pub async fn has_active_operations(&self) -> bool {
        self.operation_count().await > 0
    }

    /// Add a subscription to tracking
    pub async fn add_subscription(&self, subscription_id: String, filter_count: usize) {
        let mut subs = self.active_subscriptions.write().unwrap();
        subs.insert(subscription_id, filter_count);
        drop(subs);
        self.update_activity().await;
    }

    /// Remove a subscription from tracking
    pub async fn remove_subscription(&self, subscription_id: &str) -> bool {
        let mut subs = self.active_subscriptions.write().unwrap();
        let removed = subs.remove(subscription_id).is_some();
        drop(subs);
        if removed {
            self.update_activity().await;
        }
        removed
    }

    /// Get active subscriptions
    pub async fn active_subscriptions(&self) -> HashMap<String, usize> {
        self.active_subscriptions.read().unwrap().clone()
    }

    /// Add a publish to tracking
    pub async fn add_publish(&self, event_id: String, publish_id: String) {
        let mut pubs = self.active_publishes.write().unwrap();
        pubs.insert(event_id, publish_id);
        drop(pubs);
        self.update_activity().await;
    }

    /// Remove a publish from tracking
    pub async fn remove_publish(&self, event_id: &str) -> Option<String> {
        let mut pubs = self.active_publishes.write().unwrap();
        let removed = pubs.remove(event_id);
        drop(pubs);
        if removed.is_some() {
            self.update_activity().await;
        }
        removed
    }

    /// Get active publishes
    pub async fn active_publishes(&self) -> HashMap<String, String> {
        self.active_publishes.read().unwrap().clone()
    }

    /// Update last activity timestamp
    async fn update_activity(&self) {
        let mut activity = self.last_activity.write().unwrap();
        *activity = instant::Instant::now();
    }

    /// Get last activity timestamp
    pub async fn last_activity(&self) -> instant::Instant {
        *self.last_activity.read().unwrap()
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
        crate::relays::utils::validate_relay_url(&self.url)?;

        // Connect WebSocket
        let websocket = WebSocket::open(&self.url).map_err(|e| {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Failed;
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
            e
        })?;

        // Update status and stats
        {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Connected;
        }
        {
            let mut stats = self.stats.write().unwrap();
            stats.connected_at = Some(instant::Instant::now());
        }

        self.update_activity().await;

        tracing::debug!(relay = %self.url, "Connected to relay");
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
                    }
                }
            }
        });

        Ok(())
    }

    /// Send a message to the relay
    pub async fn send_message(&self, message: ClientMessage) -> Result<(), RelayError> {
        // Check connection status
        let status = self.status.read().unwrap();
        if !status.is_connected() {
            return Err(RelayError::ConnectionClosed);
        }
        drop(status);

        // Serialize
        let json = message.to_json().map_err(|e| {
            tracing::error!("Failed to serialize message: {}", e);
            RelayError::ProtocolError(format!("Serialization error: {}", e))
        })?;

        // Lock the sink
        let mut sink_guard = self.ws_sink.lock().await;
        let sink = sink_guard.as_mut().ok_or(RelayError::ConnectionClosed)?;

        // Try to send
        if let Err(e) = sink.send(Message::Text(json)).await {
            tracing::error!(error = %e, "Failed to send message: marking connection closed");

            // 🔹 Mark connection status immediately
            {
                let mut status_guard = self.status.write().unwrap();
                *status_guard = ConnectionStatus::Failed; // or ConnectionStatus::Closed
            }

            // 🔹 Drop sink so future sends fail fast
            *sink_guard = None;

            return Err(RelayError::ConnectionClosed);
        }

        Ok(())
    }

    /// Get receiver for incoming messages (can only be taken once)
    pub async fn take_message_receiver(&self) -> Option<mpsc::UnboundedReceiver<RelayResponse>> {
        let mut incoming_guard = self.incoming_rx.write().unwrap();
        incoming_guard.take()
    }

    /// Reconnect to the relay
    pub async fn reconnect(&self, cb: EventCallback) -> Result<(), RelayError> {
        tracing::debug!(relay = %self.url, "Reconnecting to relay");

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

    /// Check if connection should be closed due to inactivity
    pub async fn should_close_due_to_inactivity(&self) -> bool {
        // Don't close if we have active operations
        if self.has_active_operations().await {
            return false;
        }

        // Check if connection has been idle for too long
        let idle_timeout = self.config.keepalive_timeout;
        let last_activity = self.last_activity().await;
        last_activity.elapsed() > idle_timeout
    }

    /// Close the connection
    pub async fn close(&self) -> Result<(), RelayError> {
        tracing::debug!(relay = %self.url, "Closing connection");

        // Update status
        {
            let mut status = self.status.write().unwrap();
            *status = ConnectionStatus::Closed;
        }

        // Abort connection tasks
        if let Some(abort_handle) = self.connection_abort.write().unwrap().take() {
            abort_handle.abort();
        }

        // Close WebSocket
        {
            let mut ws_guard = self.websocket.write().unwrap();
            if let Some(ws) = ws_guard.take() {
                let _ = ws.close(None, None);
            }
        }

        // Clear operation tracking
        {
            let mut subs = self.active_subscriptions.write().unwrap();
            subs.clear();
        }
        {
            let mut pubs = self.active_publishes.write().unwrap();
            pubs.clear();
        }

        Ok(())
    }
}

impl Drop for RelayConnection {
    fn drop(&mut self) {
        // Spawn a task to close the connection since we can't await in Drop
        let status = self.status.clone();
        let connection_abort = self.connection_abort.clone();
        let websocket = self.websocket.clone();
        let url = self.url.clone();

        spawn_local(async move {
            tracing::debug!(relay = %url, "Dropping connection");

            // Update status
            {
                let mut status = status.write().unwrap();
                *status = ConnectionStatus::Closed;
            }

            // Abort connection tasks
            if let Some(abort_handle) = connection_abort.write().unwrap().take() {
                abort_handle.abort();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_relay_connection_creation() {
        let config = RelayConfig::default();
        let conn = RelayConnection::new("wss://relay.example.com".to_string(), config);

        assert_eq!(conn.url(), "wss://relay.example.com");
        assert_eq!(conn.status().await, ConnectionStatus::Disconnected);
        assert_eq!(conn.operation_count().await, 0);
        assert!(!conn.has_active_operations().await);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_subscription_tracking() {
        let config = RelayConfig::default();
        let conn = RelayConnection::new("wss://relay.example.com".to_string(), config);

        assert_eq!(conn.operation_count().await, 0);

        // Add subscription
        conn.add_subscription("sub1".to_string(), 2).await;
        assert_eq!(conn.operation_count().await, 1);
        assert!(conn.has_active_operations().await);

        let subs = conn.active_subscriptions().await;
        assert_eq!(subs.len(), 1);
        assert_eq!(subs.get("sub1"), Some(&2));

        // Add another subscription
        conn.add_subscription("sub2".to_string(), 1).await;
        assert_eq!(conn.operation_count().await, 2);

        // Remove subscription
        let removed = conn.remove_subscription("sub1").await;
        assert!(removed);
        assert_eq!(conn.operation_count().await, 1);

        // Remove non-existent subscription
        let not_removed = conn.remove_subscription("sub3").await;
        assert!(!not_removed);
        assert_eq!(conn.operation_count().await, 1);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_publish_tracking() {
        let config = RelayConfig::default();
        let conn = RelayConnection::new("wss://relay.example.com".to_string(), config);

        assert_eq!(conn.operation_count().await, 0);

        // Add publish
        conn.add_publish("event1".to_string(), "pub1".to_string())
            .await;
        assert_eq!(conn.operation_count().await, 1);
        assert!(conn.has_active_operations().await);

        let pubs = conn.active_publishes().await;
        assert_eq!(pubs.len(), 1);
        assert_eq!(pubs.get("event1"), Some(&"pub1".to_string()));

        // Remove publish
        let removed = conn.remove_publish("event1").await;
        assert_eq!(removed, Some("pub1".to_string()));
        assert_eq!(conn.operation_count().await, 0);
        assert!(!conn.has_active_operations().await);
    }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_mixed_operations() {
        let config = RelayConfig::default();
        let conn = RelayConnection::new("wss://relay.example.com".to_string(), config);

        // Add both subscriptions and publishes
        conn.add_subscription("sub1".to_string(), 1).await;
        conn.add_publish("event1".to_string(), "pub1".to_string())
            .await;
        assert_eq!(conn.operation_count().await, 2);

        conn.add_subscription("sub2".to_string(), 3).await;
        assert_eq!(conn.operation_count().await, 3);

        // Remove all operations
        conn.remove_subscription("sub1").await;
        conn.remove_subscription("sub2").await;
        conn.remove_publish("event1").await;
        assert_eq!(conn.operation_count().await, 0);
        assert!(!conn.has_active_operations().await);
    }

    // #[wasm_bindgen_test::wasm_bindgen_test]
    // async fn test_invalid_url() {
    //     let config = RelayConfig::default();
    //     let conn = RelayConnection::new("invalid-url".to_string(), config);

    //     let result = conn.connect().await;
    //     assert!(result.is_err());
    //     assert_eq!(conn.status().await, ConnectionStatus::Failed);
    // }

    #[wasm_bindgen_test::wasm_bindgen_test]
    async fn test_inactivity_check() {
        let mut config = RelayConfig::default();
        config.keepalive_timeout = std::time::Duration::from_millis(1); // Very short timeout
        let conn = RelayConnection::new("wss://relay.example.com".to_string(), config);

        // With active operations, should not close
        conn.add_subscription("sub1".to_string(), 1).await;
        assert!(!conn.should_close_due_to_inactivity().await);

        // Without operations, should close after timeout
        conn.remove_subscription("sub1").await;
        // Wait for timeout
        gloo_timers::future::TimeoutFuture::new(10).await;
        assert!(conn.should_close_due_to_inactivity().await);
    }
}
