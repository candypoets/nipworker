//! Connection Registry - Main Entry Point for Relay Operations
//!
//! This module provides the main interface for creating subscriptions and publishing events
//! to Nostr relays. It manages one WebSocket connection per relay URL and tracks multiple
//! subscriptions and publishes per connection.

use crate::{
    relays::{
        connection::RelayConnection,
        types::{
            ClientMessage, ConnectionStatus, RelayConfig, RelayError, RelayMessage, RelayResponse,
        },
        utils::{normalize_relay_url, validate_relay_url},
    },
    types::{PublishStatus as MainPublishStatus, RelayStatusUpdate},
    NetworkEvent, NetworkEventType,
};
use futures::channel::mpsc;
use futures::StreamExt;
use nostr::{Event, EventId, Filter};
use rustc_hash::FxHashMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tracing::{info, warn};
use wasm_bindgen_futures::spawn_local;

/// Main connection registry for managing relay operations
pub struct ConnectionRegistry {
    /// Active relay connections (one per relay URL)
    connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
    /// Global configuration
    config: RelayConfig,
    /// Active subscriptions tracker (subscription_id -> relay_urls)
    active_subscriptions: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Active publishes tracker (event_id -> relay_urls)
    active_publishes: Arc<RwLock<HashMap<EventId, Vec<String>>>>,
    /// Event receivers for subscriptions (subscription_id -> sender)
    subscription_senders: Arc<RwLock<HashMap<String, mpsc::UnboundedSender<NetworkEvent>>>>,
    /// Publish result receivers (event_id -> sender)
    publish_result_senders: Arc<RwLock<HashMap<EventId, mpsc::UnboundedSender<RelayStatusUpdate>>>>,
}

/// Handle for a subscription that allows event streaming and cancellation
pub struct SubscriptionHandle {
    subscription_id: String,
    relay_urls: Vec<String>,
    event_receiver: mpsc::UnboundedReceiver<NetworkEvent>,
    registry: Arc<ConnectionRegistry>,
}

impl SubscriptionHandle {
    /// Get the subscription ID
    pub fn id(&self) -> &str {
        &self.subscription_id
    }

    /// Get relay URLs this subscription is active on
    pub fn relay_urls(&self) -> &[String] {
        &self.relay_urls
    }

    /// Get the next event from the subscription
    pub async fn next_event(&mut self) -> Option<NetworkEvent> {
        self.event_receiver.next().await
    }

    /// Cancel the subscription
    pub async fn cancel(self) {
        let _ = self
            .registry
            .close_subscription(&self.subscription_id)
            .await;
    }
}

impl futures::Stream for SubscriptionHandle {
    type Item = NetworkEvent;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures::StreamExt;
        self.event_receiver.poll_next_unpin(cx)
    }
}

/// Handle for a publish operation that allows tracking results
pub struct PublishHandle {
    event_id: EventId,
    relay_urls: Vec<String>,
    result_receiver: mpsc::UnboundedReceiver<RelayStatusUpdate>,
    registry: Arc<ConnectionRegistry>,
}

impl PublishHandle {
    /// Get the event ID
    pub fn event_id(&self) -> &EventId {
        &self.event_id
    }

    /// Get relay URLs this publish is targeting
    pub fn relay_urls(&self) -> &[String] {
        &self.relay_urls
    }

    /// Get the next publish result
    pub async fn next_result(&mut self) -> Option<RelayStatusUpdate> {
        self.result_receiver.next().await
    }

    /// Wait for all publish results
    pub async fn wait_for_all_results(&mut self) -> Vec<RelayStatusUpdate> {
        let mut results = Vec::new();
        let expected_count = self.relay_urls.len();

        while results.len() < expected_count {
            if let Some(result) = self.next_result().await {
                results.push(result);
            } else {
                // Channel closed, break the loop
                break;
            }
        }

        results
    }

    /// Cancel the publish operation
    pub async fn cancel(self) {
        let _ = self.registry.cancel_publish(&self.event_id).await;
    }
}

impl ConnectionRegistry {
    /// Create a new connection registry
    pub fn new() -> Self {
        Self::with_config(RelayConfig::default())
    }

    /// Create a new connection registry with custom configuration
    pub fn with_config(config: RelayConfig) -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
            config,
            active_subscriptions: Arc::new(RwLock::new(HashMap::new())),
            active_publishes: Arc::new(RwLock::new(HashMap::new())),
            subscription_senders: Arc::new(RwLock::new(HashMap::new())),
            publish_result_senders: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a subscription to one or more relays
    ///
    /// This method creates a new subscription to receive events from specified relays according to
    /// the provided filters. Each subscription is identified by a unique `subscription_id` which can
    /// be used to track and close the subscription later.
    ///
    /// # Parameters
    /// * `subscription_id` - A unique identifier for this subscription
    /// * `reqs` - A mapping of relay URLs to filter sets, where each relay receives a specific set of filters
    ///
    /// # Returns
    /// * `Ok(SubscriptionHandle)` - A handle that can be used to receive events and manage the subscription
    /// * `Err(RelayError)` - If the subscription could not be created
    /// ```
    pub async fn subscribe(
        &self,
        subscription_id: String,
        reqs: FxHashMap<String, Vec<Filter>>,
    ) -> Result<SubscriptionHandle, RelayError> {
        // Check if subscription already exists
        {
            let active_subs = self.active_subscriptions.read().unwrap();
            if active_subs.contains_key(&subscription_id) {
                drop(active_subs);
                self.close_subscription(&subscription_id).await?;
            }
        }

        // Create event channel for this subscription
        let (event_sender, event_receiver) = mpsc::unbounded();

        // Store subscription tracking with all URLs initially
        let urls: Vec<String> = reqs.keys().cloned().collect();
        {
            let mut active_subs = self.active_subscriptions.write().unwrap();
            active_subs.insert(subscription_id.clone(), urls.clone());
        }
        {
            let mut senders = self.subscription_senders.write().unwrap();
            senders.insert(subscription_id.clone(), event_sender);
        }

        // Return handle immediately
        let handle = SubscriptionHandle {
            subscription_id: subscription_id.clone(),
            relay_urls: urls,
            event_receiver,
            registry: Arc::new(self.clone()),
        };

        // Spawn background task for connections
        let registry = self.clone();
        spawn_local(async move {
            for (url, filters) in reqs {
                let registry = registry.clone();
                let sub_id = subscription_id.clone();
                let url_clone = url.clone();
                let filters_clone = filters.clone();

                // Spawn individual connection attempt
                spawn_local(async move {
                    match registry.ensure_connection(&url_clone).await {
                        Ok(connection) => {
                            // Add subscription to connection tracking
                            connection
                                .add_subscription(sub_id.clone(), filters_clone.len())
                                .await;

                            // Start message processing for this connection if not already started
                            registry
                                .ensure_message_processing(url_clone.clone(), connection.clone())
                                .await;

                            // Send REQ message
                            let req_message = ClientMessage::req(sub_id.clone(), filters_clone);
                            if let Err(e) = connection.send_message(req_message).await {
                                tracing::error!(relay = %url_clone, error = %e, "Failed to send REQ message");
                                connection.remove_subscription(&sub_id).await;

                                // Remove failed URL from active subscriptions
                                if let Ok(mut active_subs) = registry.active_subscriptions.write() {
                                    if let Some(urls) = active_subs.get_mut(&sub_id) {
                                        urls.retain(|u| u != &url_clone);
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::error!(relay = %url_clone, error = %e, "Failed to connect to relay");

                            // Remove failed URL from active subscriptions
                            if let Ok(mut active_subs) = registry.active_subscriptions.write() {
                                if let Some(urls) = active_subs.get_mut(&sub_id) {
                                    urls.retain(|u| u != &url_clone);
                                }
                            }
                        }
                    }
                });
            }
        });

        Ok(handle)
    }

    /// Publish an event to one or more relays
    pub async fn publish(
        &self,
        event: Event,
        relay_urls: Vec<String>,
    ) -> Result<PublishHandle, RelayError> {
        if relay_urls.is_empty() {
            return Err(RelayError::InvalidUrl("No relay URLs provided".to_string()));
        }

        let event_id = event.id;

        // Validate and normalize URLs
        let mut normalized_urls = Vec::new();
        for url in relay_urls {
            if let Err(e) = validate_relay_url(&url) {
                warn!("Invalid relay URL {}: {}, skipping", url, e);
                continue;
            }
            normalized_urls.push(normalize_relay_url(&url));
        }

        // Log publish information
        tracing::info!(
            event_id = %event_id,
            relay_count = normalized_urls.len(),
            relays = ?normalized_urls,
            "Publishing event to relays"
        );

        // Create result channel for this publish
        let (result_sender, result_receiver) = mpsc::unbounded::<RelayStatusUpdate>();

        // Store publish tracking
        {
            let mut active_pubs = self.active_publishes.write().unwrap();
            active_pubs.insert(event_id, normalized_urls.clone());
        }
        {
            let mut senders = self.publish_result_senders.write().unwrap();
            senders.insert(event_id, result_sender.clone());
        }

        // Send pending status for all relays
        for url in &normalized_urls {
            let _ = self
                .send_publish_result(
                    event_id,
                    RelayStatusUpdate {
                        relay: url.clone(),
                        status: MainPublishStatus::Pending,
                        message: "Preparing to publish".to_string(),
                        timestamp: js_sys::Date::now() as i64,
                    },
                )
                .await;
        }

        // Ensure connections to all relays and send EVENT messages
        for url in &normalized_urls {
            let connection = self.ensure_connection(url).await?;

            // Add publish to connection tracking
            connection
                .add_publish(event_id.to_hex(), "".to_string())
                .await;

            // Start message processing for this connection if not already started
            self.ensure_message_processing(url.clone(), connection.clone())
                .await;

            // Send EVENT message
            let event_message = ClientMessage::event(event.clone());
            if let Err(e) = connection.send_message(event_message).await {
                tracing::error!(relay = %url, error = %e, "Failed to send EVENT message");
                connection.remove_publish(&event_id.to_hex()).await;

                // Send failure result
                let _ = self
                    .send_publish_result(
                        event_id,
                        RelayStatusUpdate {
                            relay: url.clone(),
                            status: MainPublishStatus::ConnectionError,
                            message: e.to_string(),
                            timestamp: js_sys::Date::now() as i64,
                        },
                    )
                    .await;
                continue;
            } else {
                // Send successful sent status
                let _ = self
                    .send_publish_result(
                        event_id,
                        RelayStatusUpdate {
                            relay: url.clone(),
                            status: MainPublishStatus::Sent,
                            message: "Event sent to relay".to_string(),
                            timestamp: js_sys::Date::now() as i64,
                        },
                    )
                    .await;
            }
        }

        Ok(PublishHandle {
            event_id,
            relay_urls: normalized_urls,
            result_receiver,
            registry: Arc::new(self.clone()),
        })
    }

    /// Close a subscription
    pub async fn close_subscription(&self, subscription_id: &str) -> Result<(), RelayError> {
        // Get relay URLs for this subscription
        let relay_urls = {
            let mut active_subs = self.active_subscriptions.write().unwrap();
            active_subs.remove(subscription_id)
        };

        if let Some(urls) = relay_urls {
            // Send CLOSE messages to all relays
            let close_message = ClientMessage::close(subscription_id.to_string());
            for url in &urls {
                if let Some(connection) = self.get_connection(url).await {
                    info!(relay = %url, subscription_id = %subscription_id, "Sending CLOSE message to relay");
                    if let Err(e) = connection.send_message(close_message.clone()).await {
                        tracing::error!(relay = %url, error = %e, "Failed to send CLOSE message");
                    }
                    connection.remove_subscription(subscription_id).await;
                }
            }

            // Remove sender
            let mut senders = self.subscription_senders.write().unwrap();
            senders.remove(subscription_id);
        }

        Ok(())
    }

    /// Cancel a publish operation
    pub async fn cancel_publish(&self, event_id: &EventId) -> Result<(), RelayError> {
        // Get relay URLs for this publish
        let relay_urls = {
            let mut active_pubs = self.active_publishes.write().unwrap();
            active_pubs.remove(event_id)
        };

        if let Some(urls) = relay_urls {
            // Remove publish tracking from connections
            for url in &urls {
                if let Some(connection) = self.get_connection(url).await {
                    connection.remove_publish(&event_id.to_hex()).await;
                }
            }

            // Remove sender
            let mut senders = self.publish_result_senders.write().unwrap();
            senders.remove(event_id);
        }

        Ok(())
    }

    /// Get or create a connection to a relay
    async fn ensure_connection(&self, url: &str) -> Result<Arc<RelayConnection>, RelayError> {
        // Check if connection already exists and is ready
        if let Some(connection) = self.get_connection(url).await {
            if let Ok(()) = connection.wait_for_ready().await {
                return Ok(connection);
            }

            // If not ready, try to reconnect
            if let Err(e) = connection.reconnect().await {
                tracing::warn!(relay = %url, error = %e, "Failed to reconnect, creating new connection");
            } else {
                return Ok(connection);
            }
        }

        // Create new connection
        let connection = Arc::new(RelayConnection::new(url.to_string(), self.config.clone()));

        // Store connection
        {
            let mut connections = self.connections.write().unwrap();
            connections.insert(url.to_string(), connection.clone());
        }

        // Connect
        connection.connect().await?;

        Ok(connection)
    }

    /// Get an existing connection
    async fn get_connection(&self, url: &str) -> Option<Arc<RelayConnection>> {
        let connections = self.connections.read().unwrap();
        connections.get(url).cloned()
    }

    /// Ensure message processing is started for a connection
    async fn ensure_message_processing(&self, url: String, connection: Arc<RelayConnection>) {
        // Try to get the message receiver (can only be taken once)
        if let Some(mut receiver) = connection.take_message_receiver().await {
            let registry = Arc::new(self.clone());
            let url_clone = url.clone();

            spawn_local(async move {
                tracing::debug!(relay = %url_clone, "Starting message processing");

                while let Some(response) = receiver.next().await {
                    if let Err(e) = registry.process_relay_message(response, url.clone()).await {
                        tracing::error!(relay = %url_clone, error = %e, "Failed to process relay message");
                    }
                }

                tracing::debug!(relay = %url_clone, "Message processing ended");
            });
        }
    }

    /// Process incoming relay message
    async fn process_relay_message(
        &self,
        response: RelayResponse,
        url: String,
    ) -> Result<(), RelayError> {
        match &response.message {
            RelayMessage::Event {
                subscription_id,
                event,
                ..
            } => {
                // Send event to subscription
                if let Err(e) = self
                    .send_event_to_subscription(
                        subscription_id,
                        NetworkEvent {
                            event_type: NetworkEventType::Event,
                            event: Some(event.clone()),
                            error: None,
                            relay: Some(url.clone()),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        subscription_id = %subscription_id,
                        error = %e,
                        "Failed to send event to subscription"
                    );
                }
            }
            RelayMessage::Ok {
                event_id,
                accepted,
                message,
                ..
            } => {
                // Send publish result
                let event_id_obj = EventId::from_hex(event_id)
                    .map_err(|e| RelayError::ProtocolError(format!("Invalid event ID: {}", e)))?;

                let result = RelayStatusUpdate {
                    relay: response.relay_url.clone(),
                    status: if *accepted {
                        MainPublishStatus::Success
                    } else {
                        MainPublishStatus::Rejected
                    },
                    message: message.clone(),
                    timestamp: js_sys::Date::now() as i64,
                };

                if let Err(e) = self.send_publish_result(event_id_obj, result).await {
                    tracing::warn!(
                        event_id = %event_id,
                        error = %e,
                        "Failed to send publish result"
                    );
                }

                // Remove publish tracking from connection
                if let Some(connection) = self.get_connection(&response.relay_url).await {
                    connection.remove_publish(event_id).await;
                }
            }
            RelayMessage::Eose {
                subscription_id, ..
            } => {
                tracing::debug!(
                    subscription_id = %subscription_id,
                    relay = %response.relay_url,
                    "Received EOSE"
                );

                // Send event to subscription
                if let Err(e) = self
                    .send_event_to_subscription(
                        subscription_id,
                        NetworkEvent {
                            event_type: NetworkEventType::EOSE,
                            event: None,
                            error: None,
                            relay: Some(url.clone()),
                        },
                    )
                    .await
                {
                    tracing::warn!(
                        subscription_id = %subscription_id,
                        error = %e,
                        "Failed to send event to subscription"
                    );
                }
            }
            RelayMessage::Closed {
                subscription_id,
                message,
                ..
            } => {
                tracing::info!(
                    subscription_id = %subscription_id,
                    relay = %response.relay_url,
                    message = %message,
                    "Subscription closed by relay"
                );

                // Remove subscription tracking from connection
                if let Some(connection) = self.get_connection(&response.relay_url).await {
                    connection.remove_subscription(subscription_id).await;
                }
            }
            RelayMessage::Notice { message, .. } => {
                tracing::info!(
                    relay = %response.relay_url,
                    message = %message,
                    "Received notice from relay"
                );
            }
        }

        Ok(())
    }

    /// Send event to subscription
    async fn send_event_to_subscription(
        &self,
        subscription_id: &str,
        network_event: NetworkEvent,
    ) -> Result<(), RelayError> {
        let senders = self.subscription_senders.read().unwrap();
        if let Some(sender) = senders.get(subscription_id) {
            tracing::debug!(
                subscription_id = %subscription_id,
                event_type = ?network_event.event_type,
                "Sending event to subscription"
            );
            sender
                .unbounded_send(network_event)
                .map_err(|_| RelayError::ConnectionClosed)?;
        }
        Ok(())
    }

    /// Send publish result
    async fn send_publish_result(
        &self,
        event_id: EventId,
        result: RelayStatusUpdate,
    ) -> Result<(), RelayError> {
        let senders = self.publish_result_senders.read().unwrap();
        if let Some(sender) = senders.get(&event_id) {
            sender
                .unbounded_send(result)
                .map_err(|_| RelayError::ConnectionClosed)?;
        }
        Ok(())
    }

    /// Get connection status for a relay
    pub async fn connection_status(&self, url: &str) -> Option<ConnectionStatus> {
        let normalized_url = normalize_relay_url(url);
        if let Some(connection) = self.get_connection(&normalized_url).await {
            Some(connection.status().await)
        } else {
            None
        }
    }

    /// Get statistics for all connections
    pub async fn connection_stats(&self) -> HashMap<String, crate::relays::types::ConnectionStats> {
        let connections = self.connections.read().unwrap();
        let mut stats = HashMap::new();

        for (url, connection) in connections.iter() {
            stats.insert(url.clone(), connection.stats().await);
        }

        stats
    }

    /// Get all active subscription IDs
    pub async fn active_subscription_ids(&self) -> Vec<String> {
        let active_subs = self.active_subscriptions.read().unwrap();
        active_subs.keys().cloned().collect()
    }

    /// Get all active publish event IDs
    pub async fn active_publish_ids(&self) -> Vec<EventId> {
        let active_pubs = self.active_publishes.read().unwrap();
        active_pubs.keys().cloned().collect()
    }

    /// Disconnect from a specific relay
    pub async fn disconnect(&self, url: &str) -> Result<(), RelayError> {
        let normalized_url = normalize_relay_url(url);

        // Remove connection from registry
        let connection = {
            let mut connections = self.connections.write().unwrap();
            connections.remove(&normalized_url)
        };

        // Close connection if it exists
        if let Some(connection) = connection {
            connection.close().await?;
        }

        Ok(())
    }

    /// Disconnect from all relays
    pub async fn disconnect_all(&self) -> Result<(), RelayError> {
        // Get all connections
        let connections = {
            let mut connections_guard = self.connections.write().unwrap();
            let connections: Vec<_> = connections_guard.drain().collect();
            connections
        };

        // Close all connections
        for (_, connection) in connections {
            if let Err(e) = connection.close().await {
                tracing::error!(error = %e, "Failed to close connection");
            }
        }

        // Clear all tracking
        {
            let mut active_subs = self.active_subscriptions.write().unwrap();
            active_subs.clear();
        }
        {
            let mut active_pubs = self.active_publishes.write().unwrap();
            active_pubs.clear();
        }
        {
            let mut senders = self.subscription_senders.write().unwrap();
            senders.clear();
        }
        {
            let mut senders = self.publish_result_senders.write().unwrap();
            senders.clear();
        }

        Ok(())
    }

    /// Clean up idle connections
    pub async fn cleanup(&self) -> Result<(), RelayError> {
        let mut idle_connections = Vec::new();

        {
            let connections = self.connections.read().unwrap();
            for (url, connection) in connections.iter() {
                if connection.should_close_due_to_inactivity().await {
                    idle_connections.push(url.clone());
                }
            }
        }

        for url in idle_connections {
            tracing::debug!(relay = %url, "Closing idle connection");
            if let Err(e) = self.disconnect(&url).await {
                tracing::error!(relay = %url, error = %e, "Failed to disconnect idle connection");
            }
        }

        Ok(())
    }

    /// Get configuration
    pub fn config(&self) -> &RelayConfig {
        &self.config
    }
}

// Implement Clone for ConnectionRegistry (needed for Arc<ConnectionRegistry>)
impl Clone for ConnectionRegistry {
    fn clone(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            config: self.config.clone(),
            active_subscriptions: self.active_subscriptions.clone(),
            active_publishes: self.active_publishes.clone(),
            subscription_senders: self.subscription_senders.clone(),
            publish_result_senders: self.publish_result_senders.clone(),
        }
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
