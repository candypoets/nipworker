# NutsCash Nostr Rust Implementation

This directory contains a Rust implementation of the NutsCash Nostr client, organized as a collection of specialized crates that provide cryptographic signing, type definitions, and core Nostr functionality.

## Overview

This Rust implementation provides a WebAssembly-compatible Nostr client with modular architecture, replacing the previous Go-based implementation while maintaining API compatibility with the existing JavaScript frontend.

## Architecture

### Crate Structure

The project is organized into specialized crates for better modularity and maintainability:

#### `nutscash-nostr-types` (`src/types/`)
Core type definitions and data structures:
- **Request**: Subscription request structure matching Go implementation
- **ParsedEvent**: Enhanced Nostr events with parsed metadata
- **EOSE**: End of Stored Events message handling
- **ProofUnion**: Cashu proof type union for V3/V4 compatibility
- **SignerMessage**: WebAssembly communication types
- **Network types**: Publishing status, relay updates, summaries

#### `nutscash-nostr-signer` (`src/signer/`)
Cryptographic operations and key management:
- **Signer trait**: Abstract interface for signing operations
- **PrivateKeySigner**: Implementation using private keys
- **SignerManager**: Manager for multiple signer types
- **NIP-04/NIP-44**: Encryption/decryption support
- **WebAssembly bindings**: JavaScript integration

#### Main Library (`src/lib.rs`)
- **NostrClient**: Main WASM-exposed client interface
- **Module integration**: Coordination between crates
- **WebAssembly exports**: JavaScript API bindings

### Other Components

- **Relay Management** (`src/relays/`): WebSocket connection handling
- **Database Layer** (`src/db/`): Event storage and indexing
- **Parser** (`src/parser/`): Event content parsing by kind
- **Network** (`src/network/`): Subscription and publishing logic

## Key Features

### Type Safety
- Rust's type system prevents runtime errors
- Compile-time verification of data structures
- Zero-cost abstractions for performance

### Cryptographic Operations
- **NIP-04** encryption/decryption (legacy)
- **NIP-44** encryption/decryption (modern)
- **Event signing** with private keys
- **Key generation** and management
- **Multiple signer types** support

### WebAssembly Integration
- **JavaScript bindings** for browser compatibility
- **MessagePack serialization** for efficient data transfer
- **Async/await support** through wasm-bindgen-futures
- **Error handling** with proper JS error types

### Event Processing
- **Type-safe event parsing** by kind
- **Custom NutsCash events** (kinds 7374, 7375, 7376, 10019, 17375)
- **Standard Nostr events** (kinds 0, 1, 3, 4, 6, 7, 9735)
- **Event validation** and integrity checking

## Supported Event Kinds

### Standard Nostr Events
- **Kind 0**: User metadata/profiles
- **Kind 1**: Text notes/posts
- **Kind 3**: Contact lists (follows)
- **Kind 4**: Encrypted direct messages
- **Kind 6**: Reposts
- **Kind 7**: Reactions (likes, etc.)
- **Kind 9735**: Zap receipts (Lightning payments)
- **Kind 10002**: Relay list metadata (NIP-65)

### NutsCash Custom Events
- **Kind 7374**: Quote events
- **Kind 7375**: Proof events (Cashu proofs)
- **Kind 7376**: Payment history
- **Kind 9321**: Nutzaps (NutsCash zaps)
- **Kind 10019**: User settings (trusted mints, pubkey)
- **Kind 17375**: Encrypted wallet events

## Dependencies

### Core Dependencies
```toml
nostr = { version = "0.29", features = ["nip04", "nip44"] }
serde = { version = "1.0", features = ["derive"] }
wasm-bindgen = { version = "0.2", features = ["serde-serialize"] }
rmp-serde = "1.1"  # MessagePack serialization
tokio = { version = "1.0", features = ["rt", "macros", "sync", "time"] }
```

### WebAssembly Dependencies
```toml
js-sys = "0.3"
web-sys = "0.3"
wasm-bindgen-futures = "0.4"
console_error_panic_hook = "0.1"
```

## Building

### Prerequisites
- Rust 1.70+
- `wasm-pack` for WebAssembly builds

### Build Individual Crates
```bash
# Build types crate
cd src/types && cargo build

# Build signer crate  
cd src/signer && cargo build

# Build main crate
cargo build
```

### Build for WebAssembly
```bash
wasm-pack build --target web --out-dir pkg
```

### Run Tests
```bash
# Test all crates
cargo test --all

# Test specific crate
cargo test -p nutscash-nostr-types
cargo test -p nutscash-nostr-signer
```

## API Usage

### Types Crate
```rust
use nutscash_nostr_types::{Request, ParsedEvent, EOSE, ProofUnion, SignerType};

// Create a subscription request
let request = Request::new(relays, filter);

// Parse a proof union
let proof = ProofUnion::new(proof_json)?;
if proof.is_v4() {
    // Handle V4 proof
}
```

### Signer Crate
```rust
use nutscash_nostr_signer::{PrivateKeySigner, SignerManagerImpl, SignerType};

// Create a private key signer
let signer = PrivateKeySigner::generate();
let pubkey = signer.get_public_key()?;

// Create and use signer manager
let mut manager = SignerManagerImpl::new();
manager.set_signer(SignerType::PrivKey, &private_key_hex)?;
manager.sign_event(&mut event)?;
```

### WebAssembly Integration
```javascript
// Import WASM module
import init, { 
    NostrClient, 
    signEvent, 
    getPublicKey, 
    setSigner 
} from './pkg/nutscash_nostr.js';

// Initialize
await init();

// Use signer functions
await setSigner('privkey', privateKeyHex);
const pubkey = await getPublicKey();

// Sign events
const signedEventBytes = await signEvent(eventBytes);
```

## Type Alignment with Go

The Rust types are designed to maintain alignment with the original Go implementation:

### Go to Rust Mapping
```go
// Go
type Request struct {
    IDs     []string     `json:"ids,omitempty"`
    Authors []string     `json:"authors,omitempty"`
    Kinds   []int        `json:"kinds,omitempty"`
    // ...
}
```

```rust
// Rust
#[derive(Serialize, Deserialize)]
pub struct Request {
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub ids: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]  
    pub authors: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub kinds: Vec<i32>,
    // ...
}
```

### MessagePack Compatibility
Both implementations use MessagePack for efficient binary serialization, ensuring data compatibility between Go and Rust components.

## Testing Strategy

### Unit Tests
Each crate includes comprehensive unit tests:
```bash
# Types crate tests
cargo test -p nutscash-nostr-types

# Signer crate tests  
cargo test -p nutscash-nostr-signer
```

### Integration Tests
Cross-crate integration testing:
```bash
cargo test --all --test '*'
```

### WebAssembly Tests
```bash
wasm-pack test --headless --chrome
```

## WebAssembly Exports

### Global Functions
- `signEvent(binaryData: Uint8Array)`: Sign a MessagePack-encoded event
- `getPublicKey()`: Get current signer's public key
- `setSigner(type: string, data?: string)`: Set the active signer

### Classes
- `NostrClient`: Main client interface
- `WasmSignerManager`: Direct signer management

## Error Handling

### Rust Error Types
```rust
#[derive(thiserror::Error)]
pub enum SignerError {
    #[error("No signer available")]
    NoSigner,
    #[error("Invalid private key format: {0}")]
    InvalidPrivateKey(String),
    // ...
}
```

### JavaScript Error Handling
```javascript
try {
    await setSigner('privkey', invalidKey);
} catch (error) {
    console.error('Signer error:', error);
}
```

## Future Enhancements

### Planned Features
1. **Hardware signers**: Support for hardware security modules
2. **Multi-signature**: Support for multi-sig operations  
3. **Key derivation**: BIP32/BIP44 key derivation
4. **Batch operations**: Efficient batch signing
5. **Threshold signatures**: Advanced cryptographic schemes

### Performance Optimizations
- SIMD crypto operations where available
- Memory pool management
- Zero-copy serialization optimizations
- WebAssembly SIMD support

## Contributing

### Code Organization
- Keep types in `nutscash-nostr-types`
- Keep crypto operations in `nutscash-nostr-signer`
- Maintain Go type compatibility
- Use comprehensive error handling

### Testing Requirements
- Unit tests for all public APIs
- Integration tests for cross-crate functionality
- WebAssembly compatibility tests
- Performance benchmarks

### Documentation
- Document all public APIs
- Include usage examples
- Maintain README files per crate
- Update integration examples

## Security Considerations

### Private Key Handling
- Keys are never logged or exposed
- Memory is zeroed after use where possible
- Constant-time operations for sensitive data
- Secure random number generation

### WebAssembly Security
- No key material in WASM memory longer than necessary
- Proper error handling to prevent information leakage
- Secure communication with JavaScript host

## Known Limitations

### Current Restrictions
- Limited NIP support (core NIPs implemented)
- WebAssembly single-threaded execution
- No persistent key storage (handled by JavaScript layer)
- Limited hardware signer support

### Browser Compatibility
- Modern browsers with WebAssembly support
- Requires TextEncoder/TextDecoder APIs
- Async/await support required

This modular architecture provides a solid foundation for a production-ready Nostr client while maintaining flexibility and security for the NutsCash ecosystem.