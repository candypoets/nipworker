//! Connection Registry - Main Entry Point for Relay Operations
//!
//! This module provides the main interface for creating subscriptions and publishing events
//! to Nostr relays. It manages one WebSocket connection per relay URL and tracks multiple
//! subscriptions and publishes per connection.

use crate::types::nostr::{Event, Filter};
use crate::utils::relay::RelayUtils;
use crate::{
    pipeline::Pipeline,
    relays::{
        connection::RelayConnection,
        types::{ClientMessage, ConnectionStatus, RelayConfig, RelayError},
        utils::{normalize_relay_url, validate_relay_url},
    },
    utils::{buffer::SharedBufferManager, js_interop::post_worker_message},
};
use futures::future::LocalBoxFuture;
use futures::lock::Mutex;
use js_sys::SharedArrayBuffer;
use rustc_hash::FxHashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::{collections::HashMap, rc::Rc};
use tracing::info;
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::spawn_local;

/// Main connection registry for managing relay operations
pub struct ConnectionRegistry {
    /// Active relay connections (one per relay URL)
    connections: Arc<RwLock<HashMap<String, Arc<RelayConnection>>>>,
    /// Global configuration
    config: RelayConfig,
    /// Active subscriptions tracker (subscription_id -> relay_urls)
    active_subscriptions: Rc<Mutex<HashMap<String, SubscriptionMeta>>>,
}

struct SubscriptionMeta {
    relay_urls: Vec<String>,
    pipeline: Rc<Mutex<Pipeline>>,
    buffer: Rc<js_sys::SharedArrayBuffer>,
    close_on_eose: bool,
}

pub type EventCallback = Rc<dyn Fn(String, &str, &str, &str) -> LocalBoxFuture<'static, ()>>;

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
            active_subscriptions: Rc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn process_incoming_message(
        &self,
        id: String,
        kind: &str,
        message: &str,
        relay_url: &str,
    ) {
        let (pipeline, buffer, relay_urls, close_on_eose) = {
            let subs = self.active_subscriptions.lock().await;
            if let Some(meta) = subs.get(&id) {
                (
                    meta.pipeline.clone(),
                    meta.buffer.clone(),
                    meta.relay_urls.clone(),
                    meta.close_on_eose,
                )
            } else {
                tracing::warn!(?id, "Message for unknown subscription");
                return;
            }
            // ⚠ subs.lock() guard is dropped here automatically
        };
        match kind {
            "EVENT" => {
                let mut pipeline_guard = pipeline.lock().await;
                if let Ok(Some(output)) = pipeline_guard.process(message).await {
                    SharedBufferManager::write_to_buffer(&buffer, &output).await;
                    // post_worker_message(&JsValue::from_str(&id));
                }
            }
            "EOSE" => {
                SharedBufferManager::send_connection_status(&buffer, relay_url, kind, message)
                    .await;
                post_worker_message(&JsValue::from_str(&id));
                if close_on_eose {
                    let _ = self.close_subscription(&id).await;
                }
            }
            _ => {
                tracing::info!(kind = %kind, relay_url = %relay_url, "Received relay message");
                // SharedBufferManager::send_connection_status(&buffer, relay_url, kind, message)
                //     .await;
            }
        }
    }

    pub async fn subscribe(
        &self,
        subscription_id: String,
        relay_filters: FxHashMap<String, Vec<Filter>>,
        pipeline: Rc<Mutex<Pipeline>>,
        buffer: Rc<SharedArrayBuffer>,
        close_on_eose: bool,
    ) -> Result<(), RelayError> {
        // Store this subscription's pipeline & buffer
        self.active_subscriptions.lock().await.insert(
            subscription_id.clone(),
            SubscriptionMeta {
                relay_urls: Vec::new(),
                pipeline,
                buffer: buffer.clone(),
                close_on_eose,
            },
        );

        for (url, filters) in relay_filters {
            let subscription_id = subscription_id.clone();
            let buffer_clone = buffer.clone();
            let self_clone = self.clone();
            let normalized_url = RelayUtils::normalize_url(url.as_str());
            spawn_local(async move {
                let conn = match self_clone.ensure_connection(&normalized_url).await {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::warn!(relay = %normalized_url, error = %e, "Failed to ensure connection, skipping");
                        SharedBufferManager::send_connection_status(
                            &buffer_clone,
                            &normalized_url,
                            "FAILED",
                            &e.to_string(),
                        )
                        .await;
                        post_worker_message(&JsValue::from_str(&subscription_id));
                        return;
                    }
                };

                // Tell the connection we have interest in this sub_id
                conn.add_subscription(subscription_id.clone(), filters.len())
                    .await;

                // Send the REQ for this subscription to this relay
                let req_message = ClientMessage::req(subscription_id.clone(), filters);
                if let Err(e) = conn.send_message(req_message).await {
                    tracing::error!(relay = %normalized_url, error = %e, "Failed to send REQ message");
                    SharedBufferManager::send_connection_status(
                        &buffer_clone,
                        &normalized_url,
                        "FAILED",
                        &e.to_string(),
                    )
                    .await;
                    post_worker_message(&JsValue::from_str(&subscription_id));
                } else {
                    SharedBufferManager::send_connection_status(
                        &buffer_clone,
                        &normalized_url,
                        "SUBSCRIBED",
                        "",
                    )
                    .await;
                    post_worker_message(&JsValue::from_str(&subscription_id));
                }
            });
        }

        Ok(())
    }

    /// Publish an event to one or more relays
    pub async fn publish(
        &self,
        publish_id: &str,
        event: Event,
        relay_urls: Vec<String>,
        buffer: Rc<SharedArrayBuffer>,
    ) -> Result<(), RelayError> {
        if relay_urls.is_empty() {
            return Err(RelayError::InvalidUrl("No relay URLs provided".to_string()));
        }

        let event_id = event.id;

        // Validate & normalize URLs (like subscribe does)
        let mut normalized_urls = Vec::new();
        for url in relay_urls {
            if let Err(e) = validate_relay_url(&url) {
                tracing::warn!("Invalid relay URL {}: {}, skipping", url, e);
                continue;
            }
            normalized_urls.push(RelayUtils::normalize_url(&url));
        }

        self.active_subscriptions.lock().await.insert(
            event_id.to_hex(),
            SubscriptionMeta {
                relay_urls: normalized_urls.clone(),
                pipeline: Rc::new(Mutex::new(Pipeline::new(vec![], "".to_string()).unwrap())),
                buffer: buffer.clone(),
                close_on_eose: false,
            },
        );

        for url in normalized_urls {
            let conn = match self.ensure_connection(&url).await {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::warn!(relay=%url, error=%e, "Failed to ensure connection, skipping");
                    continue;
                }
            };

            // Store publish tracking in the connection
            conn.add_publish(event_id.to_hex(), "".into()).await;
            SharedBufferManager::send_connection_status(&buffer, &url, "SENT", "").await;
            // Send EVENT message
            let event_message = ClientMessage::event(event.clone());
            if let Err(e) = conn.send_message(event_message).await {
                tracing::error!(relay=%url, error=%e, "Failed to send EVENT message");
                // Optionally remove from publish tracking if failed
                conn.remove_publish(&event_id.to_hex()).await;
                SharedBufferManager::send_connection_status(
                    &buffer,
                    &url,
                    "FAILED",
                    &e.to_string(),
                )
                .await;
                post_worker_message(&JsValue::from_str(publish_id));
                continue;
            } else {
                SharedBufferManager::send_connection_status(&buffer, &url, "OK", "").await;
                post_worker_message(&JsValue::from_str(publish_id));
            }
        }

        Ok(())
    }

    /// Close a subscription
    pub async fn close_subscription(&self, subscription_id: &str) -> Result<(), RelayError> {
        // Get relay URLs for this subscription
        let relay_urls = {
            let mut active_subs = self.active_subscriptions.lock().await;
            active_subs
                .remove(subscription_id)
                .map(|meta| meta.relay_urls)
        };

        if let Some(urls) = relay_urls {
            // Send CLOSE messages to all relays
            for url in &urls {
                if let Some(connection) = self.get_connection(url).await {
                    info!(relay = %url, subscription_id = %subscription_id, "Sending CLOSE message to relay");
                    let close_message = ClientMessage::close(subscription_id.to_string());
                    if let Err(e) = connection.send_message(close_message).await {
                        tracing::error!(relay = %url, error = %e, "Failed to send CLOSE message");
                    }
                    connection.remove_subscription(subscription_id).await;
                }
            }
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
            // If not ready, try to reconnect
            if let Err(e) = connection.reconnect(cb).await {
                tracing::warn!(relay = %url, error = %e, "Failed to reconnect, creating new connection");
                return Err(e);
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
        // Connect
        if let Err(e) = connection.connect(cb).await {
            tracing::error!(relay = %url, error = %e, "Failed to connect to relay");
            return Err(e);
        }

        Ok(connection)
    }

    /// Get an existing connection
    async fn get_connection(&self, url: &str) -> Option<Arc<RelayConnection>> {
        let connections = self.connections.read().unwrap();
        connections.get(url).cloned()
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
        let active_subs = self.active_subscriptions.lock().await;
        active_subs.keys().cloned().collect()
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
            let mut active_subs = self.active_subscriptions.lock().await;
            active_subs.clear();
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
        }
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
