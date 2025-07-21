# Nostr Relay Management System

This module provides a complete Rust implementation of a Nostr relay management system, designed specifically for WASM environments and optimized for browser compatibility, including Safari's unique WebSocket behavior.

## 🏗️ Architecture Overview

The relay management system consists of several key components working together to provide robust, efficient relay connectivity:

### Core Components

- **`relay.rs`** - Main relay implementation with full Nostr protocol support
- **`connection.rs`** - Unified WebSocket connection handling (WASM-compatible)
- **`relay_connection.rs`** - Individual relay connection lifecycle management
- **`relay_manager.rs`** - High-level relay pool with automatic cleanup
- **`connections_registry.rs`** - Thread-safe connection registry and statistics
- **`interfaces.rs`** - Trait definitions and common types
- **`clean_relays.rs`** - URL filtering and validation utilities

### Design Principles

1. **🌐 WASM Compatibility** - All components work seamlessly in browser environments
2. **🦋 Safari Optimization** - Special handling for Safari's WebSocket limitations and behaviors
3. **🔒 Thread Safety** - All components use async-compatible locks for safe concurrent access
4. **🛡️ Error Resilience** - Comprehensive error handling with automatic reconnection strategies
5. **♻️ Resource Management** - Automatic cleanup of idle connections and memory management

## ✨ Key Features

### 🔌 Advanced Connection Management
- **Connection Pooling**: Intelligent reuse of existing connections
- **Health Monitoring**: Continuous connection health checks and recovery
- **Timeout Handling**: Configurable timeouts with exponential backoff
- **Clean Shutdown**: Graceful connection termination and resource cleanup

### 🔗 Smart URL Processing
- **Automatic Filtering**: Removes invalid URLs (media files, local addresses, malicious content)
- **Protocol Normalization**: Handles ws/wss protocols and various URL formats
- **Security Validation**: Prevents access to local network resources
- **Format Support**: Accepts various relay URL formats and normalizes them

### 📡 Subscription Management
- **Event Filtering**: Advanced filtering with multiple criteria support
- **EOSE Handling**: Proper End-of-Stored-Events processing
- **Subscription Lifecycle**: Automatic subscription cleanup and management
- **Memory Efficient**: Optimized event routing and processing

### 🍎 Safari Compatibility Layer
- **Channel Coordination**: Uses oneshot channels for connection synchronization
- **Connection Limits**: Respects Safari's concurrent connection restrictions
- **WebSocket Optimization**: Handles Safari's unique WebSocket behaviors
- **Reduced Complexity**: Simplified state management for browser compatibility

## 🚀 Quick Start

### Basic Relay Connection

```rust
use nutscash_nostr::relays::{RelayConnectionManager, clean_relays};
use instant::Duration;

// Create a relay manager with configuration
let manager = RelayConnectionManager::new(
    Duration::from_secs(10), // connect timeout
    3, // max retries
);

// Clean and validate relay URLs
let raw_relays = vec![
    "wss://relay.damus.io".to_string(),
    "wss://relay.snort.social".to_string(),
    "wss://localhost".to_string(), // Will be filtered out
    "relay.primal.net".to_string(), // Will be normalized to wss://
];

let clean_relay_urls = clean_relays(raw_relays);
println!("Valid relays: {:?}", clean_relay_urls);

// Connect to relays
for url in clean_relay_urls {
    match manager.get_relay(&url).await {
        Ok(relay) => println!("✅ Connected to: {}", url),
        Err(e) => eprintln!("❌ Failed to connect to {}: {}", url, e),
    }
}

// Get connection statistics
let stats = manager.get_stats();
println!("📊 Active connections: {}", stats.active_connections);
println!("📈 Success rate: {}/{}", stats.successful_conns, stats.total_requests);

// Clean shutdown
manager.close().await;
```

### Publishing Events

```rust
use nostr::{EventBuilder, Keys, Kind};

// Get a relay connection
let relay = manager.get_relay("wss://relay.damus.io").await?;

// Create and sign an event
let keys = Keys::generate();
let event = EventBuilder::text_note("Hello from Rust relay manager! 🦀", [])
    .to_event(&keys)?;

// Publish with automatic OK response handling
match relay.publish(event.clone()).await {
    Ok(_) => println!("📤 Event published: {}", event.id),
    Err(e) => eprintln!("💥 Publish failed: {}", e),
}
```

### Event Subscriptions

```rust
use nostr::{Filter, Kind, Timestamp};

// Create subscription filter
let since = Timestamp::now() - Duration::from_secs(3600); // Last hour
let filter = Filter::new()
    .kinds([Kind::TextNote])
    .since(since)
    .limit(10);

// Subscribe to events
let subscription = relay.subscribe(vec![filter], vec![]).await?;

// Process incoming events
tokio::spawn(async move {
    while let Some(event) = subscription.events.recv().await {
        println!("📨 New event: {} from {}", event.event.id, event.event.pubkey);
    }
});
```

## 🔧 Configuration

### Manager Configuration

The relay manager supports extensive configuration for different use cases:

```rust
use nutscash_nostr::relays::{RelayConnectionManager, Config};

// Default configuration
let manager = RelayConnectionManager::default();

// Custom configuration
let custom_manager = RelayConnectionManager::new(
    Duration::from_secs(15),  // Longer timeout for slow networks
    5,                        // More retries for unstable connections
);

// Advanced configuration via Config struct
let config = Config {
    connect_timeout: Duration::from_secs(10),
    max_retries: 3,
    cleanup_interval: Duration::from_secs(30),
    max_concurrent_conns: 20,
    safari_optimized: true,  // Enable Safari optimizations
    debug: false,
};
```

### URL Cleaning Configuration

The URL cleaning system automatically filters dangerous or invalid URLs:

```rust
let test_urls = vec![
    "wss://valid.relay.com".to_string(),           // ✅ Valid
    "http://insecure.relay.com".to_string(),       // ❌ HTTP filtered
    "wss://localhost".to_string(),                 // ❌ Local address
    "wss://192.168.1.100".to_string(),            // ❌ Private IP
    "wss://relay.com/image.jpg".to_string(),       // ❌ Media file
    "relay.example.com".to_string(),               // ✅ Normalized to wss://
];

let clean_urls = clean_relays(test_urls);
// Result: ["wss://valid.relay.com", "wss://relay.example.com"]
```

## 📊 Connection Registry

The connection registry provides powerful lifecycle management:

```rust
// Get registry statistics
let stats = manager.get_stats();
println!("Registry Stats:");
println!("  Total connections: {}", stats.registry.total_connections);
println!("  Connected: {}", stats.registry.connected_count);
println!("  Connecting: {}", stats.registry.connecting_count);
println!("  Failed: {}", stats.registry.failed_count);

// Manual connection management
let registry = manager.get_registry();

// Get all connected relays
let connected = registry.get_connected_relays().await;
println!("Connected to {} relays", connected.len());

// Check connection status
for conn in connected {
    println!("🔗 {}: {} subscribers",
        conn.get_url(),
        conn.get_subscribers()
    );
}
```

## 🛡️ Error Handling

Comprehensive error handling with meaningful context:

```rust
use nutscash_nostr::relays::RelayError;

match manager.get_relay("wss://invalid.relay.com").await {
    Ok(relay) => { /* Connection successful */ },
    Err(e) => {
        match e.downcast_ref::<RelayError>() {
            Some(RelayError::ConnectionTimeout) => {
                println!("⏰ Connection timed out - try again later");
            },
            Some(RelayError::InvalidURL { url }) => {
                println!("🚫 Invalid URL: {}", url);
            },
            Some(RelayError::TooManyConnections) => {
                println!("🔒 Too many concurrent connections");
            },
            _ => {
                println!("💥 Unknown error: {}", e);
            }
        }
    }
}
```

## 🧪 Testing

The module includes comprehensive test coverage:

### Running Tests

```bash
# Run all relay tests
cargo test relays::

# Run specific test modules
cargo test relays::clean_relays::tests
cargo test relays::connections_registry::tests
cargo test relays::relay_connection::tests

# Run with output
cargo test relays:: -- --nocapture
```

### Test Categories

- **🧹 URL Cleaning Tests**: Validate URL filtering and normalization
- **🔗 Connection Tests**: Test connection lifecycle and error handling
- **📋 Registry Tests**: Verify registry operations and statistics
- **⚡ Manager Tests**: Integration tests for the full system
- **🍎 Safari Tests**: Browser-specific behavior validation

## 🔍 Debugging & Monitoring

### Logging Configuration

Enable detailed logging for debugging:

```rust
use tracing::{info, debug, warn, error};
use tracing_subscriber;

// Initialize logging
tracing_subscriber::fmt()
    .with_max_level(tracing::Level::DEBUG)
    .init();

// Relay operations will now log detailed information
let manager = RelayConnectionManager::new(
    Duration::from_secs(10),
    3
);
```

### Performance Monitoring

```rust
// Monitor connection performance
let stats = manager.get_stats();
println!("📈 Performance Metrics:");
println!("  Uptime: {:?}", stats.uptime);
println!("  Success rate: {:.1}%",
    (stats.successful_conns as f64 / stats.total_requests as f64) * 100.0
);
println!("  Last cleanup: {:?}", stats.last_cleanup);

// Memory usage tracking
println!("  Active connections: {}", stats.active_connections);
println!("  Registry size: {}", stats.registry.total_connections);
```

## 🚀 Advanced Usage

### Custom Relay Options

```rust
use nutscash_nostr::relays::{Relay, WithNoticeHandler, WithCustomHandler};

// Create relay with custom handlers
let notice_handler = Arc::new(|notice: String| {
    println!("📢 Notice from relay: {}", notice);
});

let custom_handler = Arc::new(|message: String| {
    println!("🔧 Custom message: {}", message);
});

let relay = Relay::connect(
    "wss://relay.example.com".to_string(),
    vec![
        Box::new(WithNoticeHandler(notice_handler)),
        Box::new(WithCustomHandler(custom_handler)),
    ]
).await?;
```


## 🔄 Migration Guide

### From Previous Versions

If migrating from an older version of the relay system:

1. **Update imports**: Module structure has been reorganized
2. **Async compatibility**: All operations are now async
3. **Error handling**: New error types with better context
4. **Configuration**: New configuration options available

### Breaking Changes

- `RelayManager` is now fully async
- Connection methods return `Result` types
- URL cleaning is now mandatory for security

## 🤝 Contributing

### Development Setup

```bash
# Clone and setup
git clone <repository>
cd nutscash/src/model/nostr-rust

# Run tests
cargo test --lib relays::

# Check formatting
cargo fmt --check

# Lint code
cargo clippy -- -D warnings
```

### Code Style

- Use `async/await` throughout
- Implement proper error handling with context
- Add comprehensive tests for new features
- Document public APIs thoroughly
- Follow Rust naming conventions

## 📋 Current Status

### ✅ Completed Features
- ✅ Basic relay connection management
- ✅ URL cleaning and validation
- ✅ Connection registry with statistics
- ✅ WASM-compatible WebSocket handling
- ✅ Safari-optimized connection coordination
- ✅ Automatic cleanup routines
- ✅ Comprehensive error handling
- ✅ Thread-safe operations
- ✅ Event publishing and subscriptions
- ✅ Connection pooling and reuse

### 🔄 In Progress
- 🔄 Advanced subscription optimization
- 🔄 Connection multiplexing
- 🔄 Enhanced metrics collection
- 🔄 Performance optimizations

### 📅 Planned Features
- ❌ NIP-42 (Authentication) support
- ❌ NIP-11 (Relay Information) integration
- ❌ Rate limiting and backoff strategies
- ❌ Advanced monitoring dashboard
- ❌ Connection pre-warming
- ❌ Intelligent relay selection algorithms

## 📚 Dependencies

### Core Dependencies
- **`tokio`** (1.0+) - Async runtime and utilities
- **`nostr`** (0.29+) - Nostr protocol implementation
- **`anyhow`** (1.0+) - Error handling
- **`serde`** (1.0+) - Serialization support
- **`tracing`** (0.1+) - Structured logging

### WASM Dependencies
- **`tokio-tungstenite-wasm`** (0.5+) - Unified WebSocket support
- **`wasm-bindgen`** (0.2+) - JavaScript interop
- **`web-sys`** (0.3+) - Browser API bindings

### Development Dependencies
- **`tokio-test`** - Async testing utilities
- **`criterion`** - Benchmarking framework

## 📄 License

This code is part of the NutsCash project and follows the same license terms.

---

**Built with ❤️ for the Nostr ecosystem**

For more information, see the [NutsCash project documentation](../../README.md).
