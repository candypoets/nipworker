//! Types for Nostr Relay Protocol (NIP-01)
//!
//! This module defines the message types used in the Nostr relay protocol
//! as specified in NIP-01, along with connection and operation status types.

use futures::future::AbortHandle;
use nostr::{Event, EventId, Filter};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Client-to-relay messages as defined in NIP-01
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RelayMessage {
    /// ["EVENT", <subscription_id>, <event JSON>] - Event from subscription
    Event {
        #[serde(rename = "0")]
        message_type: String, // "EVENT"
        #[serde(rename = "1")]
        subscription_id: String,
        #[serde(rename = "2")]
        event: Event,
    },

    /// ["OK", <event_id>, <true|false>, <message>] - Response to EVENT
    Ok {
        #[serde(rename = "0")]
        message_type: String, // "OK"
        #[serde(rename = "1")]
        event_id: String,
        #[serde(rename = "2")]
        accepted: bool,
        #[serde(rename = "3")]
        message: String,
    },

    /// ["EOSE", <subscription_id>] - End of stored events
    Eose {
        #[serde(rename = "0")]
        message_type: String, // "EOSE"
        #[serde(rename = "1")]
        subscription_id: String,
    },

    /// ["CLOSED", <subscription_id>, <message>] - Subscription closed by relay
    Closed {
        #[serde(rename = "0")]
        message_type: String, // "CLOSED"
        #[serde(rename = "1")]
        subscription_id: String,
        #[serde(rename = "2")]
        message: String,
    },

    /// ["NOTICE", <message>] - Human-readable notice
    Notice {
        #[serde(rename = "0")]
        message_type: String, // "NOTICE"
        #[serde(rename = "1")]
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
#[derive(Debug, Clone)]
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

    /// Serialize to JSON array format as per NIP-01
    pub fn to_json(&self) -> Result<String, RelayError> {
        match self {
            ClientMessage::Event(event) => {
                let array = serde_json::json!(["EVENT", event]);
                serde_json::to_string(&array).map_err(RelayError::SerializeError)
            }
            ClientMessage::Req {
                subscription_id,
                filters,
            } => {
                let mut array = vec![
                    serde_json::Value::String("REQ".to_string()),
                    serde_json::Value::String(subscription_id.clone()),
                ];
                for filter in filters {
                    array.push(serde_json::to_value(filter).map_err(RelayError::SerializeError)?);
                }
                serde_json::to_string(&array).map_err(RelayError::SerializeError)
            }
            ClientMessage::Close { subscription_id } => {
                let array = serde_json::json!(["CLOSE", subscription_id]);
                serde_json::to_string(&array).map_err(RelayError::SerializeError)
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

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::{EventBuilder, Keys, Kind};

    #[test]
    fn test_client_message_serialization() {
        // Test EVENT message
        let keys = Keys::generate();
        let event = EventBuilder::text_note("Hello world!", [])
            .to_event(&keys)
            .unwrap();
        let msg = ClientMessage::event(event.clone());
        let json = msg.to_json().unwrap();
        assert!(json.contains("EVENT"));
        assert!(json.contains(&event.id.to_hex()));

        // Test REQ message
        let filter = Filter::new().kinds([Kind::TextNote]).limit(10);
        let msg = ClientMessage::req("sub1".to_string(), vec![filter]);
        let json = msg.to_json().unwrap();
        assert!(json.contains("REQ"));
        assert!(json.contains("sub1"));

        // Test CLOSE message
        let msg = ClientMessage::close("sub1".to_string());
        let json = msg.to_json().unwrap();
        assert!(json.contains("CLOSE"));
        assert!(json.contains("sub1"));
    }

    #[test]
    fn test_relay_message_parsing() {
        // Test EVENT message
        let event_json = r#"["EVENT", "sub1", {"id": "abc123", "pubkey": "def456", "created_at": 1234567890, "kind": 1, "tags": [], "content": "hello", "sig": "789abc"}]"#;
        let parsed = RelayMessage::from_json(event_json);
        assert!(parsed.is_ok());
        if let Ok(RelayMessage::Event {
            subscription_id, ..
        }) = parsed
        {
            assert_eq!(subscription_id, "sub1");
        }

        // Test OK message
        let ok_json = r#"["OK", "abc123", true, ""]"#;
        let parsed = RelayMessage::from_json(ok_json);
        assert!(parsed.is_ok());
        if let Ok(RelayMessage::Ok {
            event_id, accepted, ..
        }) = parsed
        {
            assert_eq!(event_id, "abc123");
            assert!(accepted);
        }

        // Test EOSE message
        let eose_json = r#"["EOSE", "sub1"]"#;
        let parsed = RelayMessage::from_json(eose_json);
        assert!(parsed.is_ok());
        if let Ok(RelayMessage::Eose {
            subscription_id, ..
        }) = parsed
        {
            assert_eq!(subscription_id, "sub1");
        }

        // Test NOTICE message
        let notice_json = r#"["NOTICE", "This is a notice"]"#;
        let parsed = RelayMessage::from_json(notice_json);
        assert!(parsed.is_ok());
        if let Ok(RelayMessage::Notice { message, .. }) = parsed {
            assert_eq!(message, "This is a notice");
        }
    }

    #[test]
    fn test_connection_status() {
        assert!(ConnectionStatus::Connected.is_connected());
        assert!(!ConnectionStatus::Disconnected.is_connected());

        assert!(ConnectionStatus::Disconnected.can_reconnect());
        assert!(ConnectionStatus::Failed.can_reconnect());
        assert!(!ConnectionStatus::Connecting.can_reconnect());
    }

    #[test]
    fn test_message_accessors() {
        let event_json = r#"["EVENT", "sub1", {"id": "abc123", "pubkey": "def456", "created_at": 1234567890, "kind": 1, "tags": [], "content": "hello", "sig": "789abc"}]"#;
        let msg = RelayMessage::from_json(event_json).unwrap();
        assert_eq!(msg.subscription_id(), Some("sub1"));
        assert_eq!(msg.event_id(), None);

        let ok_json = r#"["OK", "abc123", true, ""]"#;
        let msg = RelayMessage::from_json(ok_json).unwrap();
        assert_eq!(msg.subscription_id(), None);
        assert_eq!(msg.event_id(), Some("abc123"));
    }
}
