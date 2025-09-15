//! Simplified Nostr Relay Module
//!
//! This module provides a direct implementation of the Nostr relay protocol (NIP-01)
//! without complex abstractions. It supports:
//!
//! - Direct WebSocket connections using gloo-net
//! - Subscription management with cancellation via AbortHandle
//! - Event publishing with status tracking
//! - Connection lifecycle management
//! - Automatic reconnection and cleanup

pub mod connection;
pub mod connection_registry;
pub mod types;

// Re-export main public API
pub use connection::RelayConnection;
pub use connection_registry::ConnectionRegistry;
pub use types::{
    ClientMessage, ConnectionStatus, PublishStatus, RelayError, RelayMessage, RelayResponse,
    SubscriptionStatus,
};

// Re-export nostr types for convenience
pub use crate::types::nostr::{Event, EventId, Filter, PublicKey};

/// Main entry point for relay operations
///
/// # Example
///
/// ```rust
/// use nutscash_crate::types::nostr::relays::{ConnectionRegistry, SubscriptionHandle};
/// use crate::types::nostr::{Filter, Kind};
/// use futures::StreamExt;
///
/// async fn example() -> Result<(), Box<dyn std::error::Error>> {
///     let registry = ConnectionRegistry::new();
///
///     // Create a subscription
///     let filter = Filter::new().kinds([Kind::TextNote]).limit(10);
///     let subscription = registry.subscribe(
///         "my-sub-1".to_string(),
///         vec![filter],
///         vec!["wss://relay.damus.io".to_string()]
///     ).await?;
///
///     // Listen for events
///     let mut events = subscription.events();
///     while let Some(event) = events.next().await {
///         println!("Received event: {}", event.id);
///     }
///
///     Ok(())
/// }
/// ```
pub type Registry = ConnectionRegistry;

/// Create a new connection registry instance
pub fn new_registry() -> ConnectionRegistry {
    ConnectionRegistry::new()
}

/// Utility functions for the relay module
pub mod utils {
    const BLACKLISTED_RELAYS: &[&str] = &["wheat.happytavern.co"];

    use super::types::{ClientMessage, RelayError, RelayMessage};

    /// Parse a relay message from JSON string
    // pub fn parse_relay_message(json: &str) -> Result<RelayMessage, RelayError> {
    //     serde_json::from_str(json).map_err(RelayError::ParseError)
    // }

    /// Serialize a client message to JSON string
    // pub fn serialize_client_message(msg: &ClientMessage) -> Result<String, RelayError> {
    //     serde_json::to_string(msg).map_err(RelayError::SerializeError)
    // }

    /// Validate relay URL format
    pub fn validate_relay_url(url: &str) -> Result<(), RelayError> {
        if url.is_empty() {
            return Err(RelayError::InvalidUrl("URL cannot be empty".to_string()));
        }

        let normalized_url = url.trim().to_lowercase();
        for &blacklisted in BLACKLISTED_RELAYS {
            if normalized_url.contains(blacklisted) {
                return Err(RelayError::InvalidUrl(format!(
                    "Relay URL is blacklisted: {}",
                    url
                )));
            }
        }

        if !url.starts_with("ws://") && !url.starts_with("wss://") {
            return Err(RelayError::InvalidUrl(
                "URL must start with ws:// or wss://".to_string(),
            ));
        }

        Ok(())
    }

    /// Normalize relay URL (remove trailing slash, convert to lowercase)
    pub fn normalize_relay_url(url: &str) -> String {
        let mut normalized = url.trim().to_lowercase();
        if normalized.ends_with('/') && normalized.len() > 1 {
            normalized.pop();
        }
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::utils::*;

    #[test]
    fn test_validate_relay_url() {
        assert!(validate_relay_url("wss://relay.damus.io").is_ok());
        assert!(validate_relay_url("ws://localhost:8080").is_ok());
        assert!(validate_relay_url("").is_err());
        assert!(validate_relay_url("http://relay.example.com").is_err());
        assert!(validate_relay_url("invalid-url").is_err());
    }

    #[test]
    fn test_normalize_relay_url() {
        assert_eq!(
            normalize_relay_url("WSS://RELAY.DAMUS.IO/"),
            "wss://relay.damus.io"
        );
        assert_eq!(
            normalize_relay_url("wss://relay.example.com"),
            "wss://relay.example.com"
        );
        assert_eq!(
            normalize_relay_url("  WS://LOCALHOST:8080/  "),
            "ws://localhost:8080"
        );
    }
}
