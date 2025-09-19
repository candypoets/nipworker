//! Types for Nostr Relay Protocol (NIP-01)
//!
//! This module defines the message types used in the Nostr relay protocol
//! as specified in NIP-01, along with connection and operation status types.

use crate::types::nostr::{Event, EventId, Filter};
use futures::future::AbortHandle;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Client-to-relay messages as defined in NIP-01
pub enum ClientMessage {
    /// ["EVENT", <event JSON>] - Publish an event
    Event(Event),

    /// ["REQ", <subscription_id>, <filters1>, <filters2>, ...] - Create subscription
    Req {
        subscription_id: String,
        filters: Vec<Filter>,
    },

    /// ["CLOSE", <subscription_id>] - Close subscription
    Close { subscription_id: String },
}

/// Relay-to-client messages as defined in NIP-01
pub enum RelayMessage {
    /// ["EVENT", <subscription_id>, <event JSON>] - Event from subscription
    Event {
        message_type: String, // "EVENT"
        subscription_id: String,
        event: Event,
    },

    /// ["OK", <event_id>, <true|false>, <message>] - Response to EVENT
    Ok {
        message_type: String, // "OK"
        event_id: String,
        accepted: bool,
        message: String,
    },

    /// ["EOSE", <subscription_id>] - End of stored events
    Eose {
        message_type: String, // "EOSE"
        subscription_id: String,
    },

    /// ["CLOSED", <subscription_id>, <message>] - Subscription closed by relay
    Closed {
        message_type: String, // "CLOSED"
        subscription_id: String,
        message: String,
    },

    /// ["NOTICE", <message>] - Human-readable notice
    Notice {
        message_type: String, // "NOTICE"
        message: String,
    },
}

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

/// Response from a relay operation
pub struct RelayResponse {
    pub relay_url: String,
    pub message: RelayMessage,
    pub timestamp: instant::Instant,
}

/// Error types for relay operations
#[derive(Debug, Error)]
pub enum RelayError {
    #[error("WebSocket error: {0}")]
    WebSocketError(String),

    #[error("Connection error: {0}")]
    ConnectionError(String),

    #[error("Parse error: {0}")]
    ParseError(#[from] serde_json::Error),

    #[error("Serialize error: {0}")]
    SerializeError(serde_json::Error),

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
    pub connected_at: Option<instant::Instant>,
}

impl ConnectionStats {
    pub fn uptime(&self) -> Option<std::time::Duration> {
        self.connected_at.map(|start| start.elapsed())
    }
}

/// Handle for cancelling operations
#[derive(Debug)]
pub struct CancellationToken {
    pub(crate) abort_handle: AbortHandle,
}

impl CancellationToken {
    /// Create a new cancellation token
    pub fn new(abort_handle: AbortHandle) -> Self {
        Self { abort_handle }
    }

    /// Cancel the operation
    pub fn cancel(&self) {
        self.abort_handle.abort();
    }

    /// Check if the operation was cancelled
    pub fn is_cancelled(&self) -> bool {
        self.abort_handle.is_aborted()
    }
}

/// Utility functions for message handling
impl ClientMessage {
    /// Create an EVENT message
    pub fn event(event: Event) -> Self {
        Self::Event(event)
    }

    /// Create a REQ message
    pub fn req(subscription_id: String, filters: Vec<Filter>) -> Self {
        Self::Req {
            subscription_id,
            filters,
        }
    }

    /// Create a CLOSE message
    pub fn close(subscription_id: String) -> Self {
        Self::Close { subscription_id }
    }

    pub fn to_json(&self) -> Result<String, RelayError> {
        match self {
            ClientMessage::Event(event) => {
                let event_json = event.as_json();
                Ok(format!(r#"["EVENT",{}]"#, event_json))
            }
            ClientMessage::Req {
                subscription_id,
                filters,
            } => {
                let mut parts = vec![format!(r#""REQ""#), format!(r#""{}""#, subscription_id)];
                for filter in filters {
                    parts.push(filter.as_json());
                }
                Ok(format!("[{}]", parts.join(",")))
            }
            ClientMessage::Close { subscription_id } => {
                Ok(format!(r#"["CLOSE","{}"]"#, subscription_id))
            }
        }
    }
}

impl RelayMessage {
    /// Parse from JSON array format as per NIP-01
    pub fn from_json(json: &str) -> Result<Self, RelayError> {
        let value: serde_json::Value = serde_json::from_str(json)?;
        let array = value.as_array().ok_or(RelayError::InvalidMessage)?;

        if array.is_empty() {
            return Err(RelayError::InvalidMessage);
        }

        let message_type = array[0].as_str().ok_or(RelayError::InvalidMessage)?;

        match message_type {
            "EVENT" => {
                if array.len() != 3 {
                    return Err(RelayError::InvalidMessage);
                }
                let subscription_id = array[1]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                let event: Event = serde_json::from_value(array[2].clone())?;
                Ok(RelayMessage::Event {
                    message_type: "EVENT".to_string(),
                    subscription_id,
                    event,
                })
            }
            "OK" => {
                if array.len() != 4 {
                    return Err(RelayError::InvalidMessage);
                }
                let event_id = array[1]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                let accepted = array[2].as_bool().ok_or(RelayError::InvalidMessage)?;
                let message = array[3]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                Ok(RelayMessage::Ok {
                    message_type: "OK".to_string(),
                    event_id,
                    accepted,
                    message,
                })
            }
            "EOSE" => {
                if array.len() != 2 {
                    return Err(RelayError::InvalidMessage);
                }
                let subscription_id = array[1]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                Ok(RelayMessage::Eose {
                    message_type: "EOSE".to_string(),
                    subscription_id,
                })
            }
            "CLOSED" => {
                if array.len() != 3 {
                    return Err(RelayError::InvalidMessage);
                }
                let subscription_id = array[1]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                let message = array[2]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                Ok(RelayMessage::Closed {
                    message_type: "CLOSED".to_string(),
                    subscription_id,
                    message,
                })
            }
            "NOTICE" => {
                if array.len() != 2 {
                    return Err(RelayError::InvalidMessage);
                }
                let message = array[1]
                    .as_str()
                    .ok_or(RelayError::InvalidMessage)?
                    .to_string();
                Ok(RelayMessage::Notice {
                    message_type: "NOTICE".to_string(),
                    message,
                })
            }
            _ => Err(RelayError::ProtocolError(format!(
                "Unknown message type: {}",
                message_type
            ))),
        }
    }

    /// Get the subscription ID if this is a subscription-related message
    pub fn subscription_id(&self) -> Option<&str> {
        match self {
            RelayMessage::Event {
                subscription_id, ..
            }
            | RelayMessage::Eose {
                subscription_id, ..
            }
            | RelayMessage::Closed {
                subscription_id, ..
            } => Some(subscription_id),
            _ => None,
        }
    }

    /// Get the event ID if this is an OK message
    pub fn event_id(&self) -> Option<&str> {
        match self {
            RelayMessage::Ok { event_id, .. } => Some(event_id),
            _ => None,
        }
    }
}
