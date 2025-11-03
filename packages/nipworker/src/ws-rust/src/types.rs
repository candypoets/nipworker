//! Types for Nostr Relay Protocol (NIP-01)
//!
//! This module defines the message types used in the Nostr relay protocol
//! as specified in NIP-01, along with connection and operation status types.

use thiserror::Error;

/// Connection status for a relay
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConnectionStatus {
    /// Not connected
    Disconnected,
    /// Connection in progress
    Connecting,
    /// Connected and ready
    Connected,
    /// Connection failed
    Failed,
    /// Connection was closed
    Closed,
}

impl ConnectionStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, ConnectionStatus::Connected)
    }

    pub fn can_reconnect(&self) -> bool {
        matches!(
            self,
            ConnectionStatus::Disconnected | ConnectionStatus::Failed | ConnectionStatus::Closed
        )
    }
}

/// Status of a subscription
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    /// Subscription is pending connection
    Pending,
    /// Subscription is active
    Active,
    /// End of stored events received
    EndOfStoredEvents,
    /// Subscription was closed
    Closed,
    /// Subscription failed
    Failed,
    /// Subscription was cancelled
    Cancelled,
}

/// Status of a publish operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishStatus {
    /// Publish is pending connection
    Pending,
    /// Event was sent to relay
    Sent,
    /// Event was accepted by relay
    Accepted,
    /// Event was rejected by relay
    Rejected,
    /// Publish failed due to connection error
    Failed,
    /// Publish was cancelled
    Cancelled,
}

/// Error types for relay operations
#[derive(Debug, Error)]
pub enum RelayError {
    #[error("WebSocket error: {0}")]
    WebSocketError(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Connection timeout")]
    Timeout,

    #[error("Operation cancelled")]
    Cancelled,

    #[error("Subscription not found: {0}")]
    SubscriptionNotFound(String),

    #[error("Publish not found: {0}")]
    PublishNotFound(String),

    #[error("Connection closed")]
    ConnectionClosed,

    #[error("Queue full")]
    QueueFull,

    #[error("Invalid message format")]
    InvalidMessage,

    #[error("Relay rejected operation: {0}")]
    RelayRejected(String),

    #[error("Protocol error: {0}")]
    ProtocolError(String),
}

impl From<gloo_net::websocket::WebSocketError> for RelayError {
    fn from(err: gloo_net::websocket::WebSocketError) -> Self {
        RelayError::WebSocketError(err.to_string())
    }
}

/// Configuration for relay connections
#[derive(Debug, Clone)]
pub struct RelayConfig {
    /// Timeout for connection attempts
    pub connect_timeout: std::time::Duration,
    /// Timeout for ping/pong keepalive
    pub keepalive_timeout: std::time::Duration,
    /// Maximum number of reconnection attempts
    pub max_reconnect_attempts: usize,
    /// Delay between reconnection attempts
    pub reconnect_delay: std::time::Duration,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            connect_timeout: std::time::Duration::from_secs(10),
            keepalive_timeout: std::time::Duration::from_secs(60),
            max_reconnect_attempts: 3,
            reconnect_delay: std::time::Duration::from_secs(2),
        }
    }
}

/// Statistics for a relay connection
#[derive(Debug, Clone, Default)]
pub struct ConnectionStats {
    /// Number of events received
    pub events_received: usize,
    /// Number of events published
    pub events_published: usize,
    /// Number of subscriptions created
    pub subscriptions_created: usize,
    /// Number of active subscriptions
    pub active_subscriptions: usize,
    /// Number of active publishes
    pub active_publishes: usize,
    /// Number of reconnection attempts
    pub reconnect_attempts: usize,
    /// Connection uptime
    pub connected_at: Option<u64>,
}
