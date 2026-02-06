# PRD: NIPWorker Rust Core Migration

## Introduction

Migrate NIPWorker from a browser-focused WASM architecture to a pure Rust core library that can be compiled for multiple platforms (server, iOS, Android, and future desktop targets). The current implementation uses 4 Web Workers with SharedArrayBuffer ring buffers for IPC - this PRD specifies how to preserve this multi-worker architecture while replacing browser-specific APIs with portable Rust equivalents.

## Goals

- Create a platform-agnostic Rust core library (`nipworker-core`)
- Preserve the 4-worker architecture (connections, cache, parser, crypto) using Tokio tasks/threads
- Replace SharedArrayBuffer IPC with Tokio channels (mpsc/broadcast)
- Replace IndexedDB with SQLite via sqlx
- Replace `gloo-net` WebSockets with `tokio-tungstenite` (native) + platform traits
- Maintain API compatibility where possible for easier TypeScript binding migration
- Enable UniFFI bindings for iOS/Android without changing core logic
- Keep WASM as a secondary target (compile core to wasm32 with wasm-bindgen-futures)

## User Stories

### US-001: Create workspace structure for Rust core
**Description:** As a developer, I want a clean workspace structure that separates the portable core from platform-specific bindings.

**Acceptance Criteria:**
- [ ] Create `crates/nipworker-core/` with lib crate
- [ ] Create `crates/nipworker-server/` for server binary
- [ ] Keep existing `src/` WASM code untouched during migration
- [ ] Define clear public API in `crates/nipworker-core/src/lib.rs`
- [ ] Cargo.toml workspace configuration with all crates
- [ ] CI workflow builds all targets

### US-002: Implement channel-based IPC replacing SAB rings
**Description:** As a developer, I need to replace SharedArrayBuffer ring buffers with portable channels for worker communication.

**Acceptance Criteria:**
- [ ] Create `ipc/` module with channel definitions
- [ ] Implement request/response channels for each worker pair:
  - parser ↔ crypto
  - parser ↔ cache
  - parser ↔ connections
  - crypto ↔ connections (for NIP-46)
- [ ] Port `SabRing` framing logic to `ChannelMessage` with same FlatBuffers serialization
- [ ] Use `tokio::sync::mpsc` for unidirectional SPSC channels
- [ ] Use `tokio::sync::broadcast` for status/events
- [ ] Benchmark channel throughput vs SAB rings (document regression if >10%)
- [ ] Typecheck/lint passes

### US-003: Port crypto worker to native Rust
**Description:** As a developer, I want the crypto worker to run as a native Tokio task with all signing operations.

**Acceptance Criteria:**
- [ ] Port `crypto/src/lib.rs` to `core/src/workers/crypto.rs`
- [ ] Remove `wasm-bindgen`, `js-sys` dependencies
- [ ] Replace `spawn_local` with `tokio::spawn`
- [ ] Implement NIP-07 as trait (browser extension not available natively)
- [ ] Keep NIP-46, NIP-04, NIP-44, private key signers
- [ ] Replace `gloo-timers` with `tokio::time::sleep`
- [ ] Unit tests for all signer types
- [ ] Typecheck/lint passes

### US-004: Port connections worker with pluggable transport
**Description:** As a developer, I need WebSocket connections that work on both native (tokio-tungstenite) and mobile (platform WS APIs).

**Acceptance Criteria:**
- [ ] Create `transport/` module with `WebSocketTransport` trait
- [ ] Implement `TokioWebSocket` using `tokio-tungstenite` + `native-tls`/`rustls`
- [ ] Port connection registry and relay management
- [ ] Replace `gloo_net::WebSocket` with trait-based transport
- [ ] Connection pooling and reconnection logic preserved
- [ ] Status updates via broadcast channel (not callback closures)
- [ ] Unit tests with mock transport
- [ ] Typecheck/lint passes

### US-005: Port cache worker with SQLite backend
**Description:** As a developer, I need to replace IndexedDB with SQLite for event caching.

**Acceptance Criteria:**
- [ ] Create `storage/` module with `EventStore` trait
- [ ] Implement `SqliteStore` using `sqlx` with `runtime-tokio`
- [ ] Port IndexedDB schema to SQLite tables:
  - events table (id, pubkey, kind, created_at, content, sig, raw_json)
  - tags table (event_id, key, value, index)
  - indices for query optimization
- [ ] Port ring buffer and sharded storage logic
- [ ] Support migrations via `sqlx migrate`
- [ ] Query builder for Nostr filters
- [ ] Benchmark cache operations vs IndexedDB (document regression if >20%)
- [ ] Typecheck/lint passes

### US-006: Port parser worker with pipeline
**Description:** As a developer, I need the parser worker to process events through the same pipeline stages.

**Acceptance Criteria:**
- [ ] Port all kind parsers (0, 1, 3, 4, 6, 7, 10002, etc.)
- [ ] Port pipeline framework (pipes, filters)
- [ ] Replace `js_sys::SharedArrayBuffer` inputs with channel receivers
- [ ] FlatBuffers serialization preserved for efficiency
- [ ] Proof verification integration with crypto worker
- [ ] Unit tests for each kind parser
- [ ] Typecheck/lint passes

### US-007: Create orchestrator managing 4 workers
**Description:** As a developer, I need a main orchestrator that spawns and coordinates the 4 workers.

**Acceptance Criteria:**
- [ ] Create `NostrClient` struct as main API
- [ ] Spawn 4 Tokio tasks on `new()`:
  - `connections_worker`
  - `cache_worker`
  - `parser_worker`
  - `crypto_worker`
- [ ] Graceful shutdown on drop (abort handles or cancellation tokens)
- [ ] Configuration struct for worker parameters
- [ ] Event subscription API (streams/broadcast)
- [ ] Publish API
- [ ] Signer configuration API
- [ ] Typecheck/lint passes

### US-008: Create server binary with HTTP/WebSocket gateway
**Description:** As a server operator, I want a standalone binary exposing Nostr client via HTTP/WebSocket.

**Acceptance Criteria:**
- [ ] Create `crates/nipworker-server/src/main.rs`
- [ ] HTTP API for publishing events
- [ ] WebSocket endpoint for real-time subscriptions
- [ ] Configuration via TOML or env vars
- [ ] Docker image build
- [ ] Prometheus metrics endpoint
- [ ] Health check endpoint
- [ ] Typecheck/lint passes

### US-009: Define UniFFI interface for mobile bindings
**Description:** As a mobile developer, I need UniFFI-compatible interfaces for iOS/Android.

**Acceptance Criteria:**
- [ ] Create `crates/nipworker-ffi/` with UniFFI setup
- [ ] Define UDL file with core types
- [ ] Expose `NostrClient` methods via FFI
- [ ] Handle async with UniFFI async support
- [ ] iOS framework generation script
- [ ] Android AAR generation script
- [ ] Example iOS app consuming the library
- [ ] Example Android app consuming the library
- [ ] Typecheck/lint passes

### US-010: Maintain WASM compatibility
**Description:** As a web developer, I want the core library to still compile to WASM.

**Acceptance Criteria:**
- [ ] Add `wasm32-unknown-unknown` target to CI
- [ ] Feature gate non-WASM code (`#[cfg(not(target_arch = "wasm32"))]`)
- [ ] WASM-specific implementations for:
  - WebSocket (keep gloo-net)
  - Storage (keep IndexedDB via indexed_db_futures)
  - Runtime (wasm-bindgen-futures)
- [ ] Conditional compilation for Tokio vs wasm-bindgen-futures
- [ ] Verify existing WASM tests still pass
- [ ] Typecheck/lint passes

## Functional Requirements

### Core Architecture

- FR-1: The core crate must compile on stable Rust 1.70+
- FR-2: The core crate must support tokio (native) and wasm-bindgen-futures (WASM) via feature flags
- FR-3: All 4 workers (connections, cache, parser, crypto) must run concurrently
- FR-4: Workers communicate via channels, not shared memory
- FR-5: FlatBuffers remain the serialization format for cross-worker messages
- FR-6: The public API must be object-safe for FFI binding generation

### Crypto Worker

- FR-7: Support private key signer (hex/nsec)
- FR-8: Support NIP-46 remote signer (bunker and QR modes)
- FR-9: Support NIP-04 encryption/decryption
- FR-10: Support NIP-44 encryption/decryption
- FR-11: Support Cashu proof verification
- FR-12: NIP-07 is browser-only (not available in native core)

### Connections Worker

- FR-13: Support WebSocket connections to multiple relays
- FR-14: Automatic reconnection with exponential backoff
- FR-15: Connection pooling (configurable max connections)
- FR-16: Subscribe/Close subscription management
- FR-17: Frame queuing with backpressure handling
- FR-18: Transport trait for platform-specific WebSocket implementations

### Cache Worker

- FR-19: SQLite schema supporting all Nostr event fields
- FR-20: Query by ids, authors, kinds, tags, time range
- FR-21: Relay list (kind 10002) caching for relay selection
- FR-22: Configurable cache size limits
- FR-23: Event deduplication by id
- FR-24: Async query interface returning streams

### Parser Worker

- FR-25: Parse all supported event kinds (NIP-01, NIP-02, NIP-04, NIP-18, NIP-25, NIP-51, NIP-57, NIP-60, NIP-61, NIP-65)
- FR-26: Pipeline with configurable pipes (filter, parse, verify, save)
- FR-27: Proof verification delegation to crypto worker
- FR-28: FlatBuffers event serialization for efficiency

### Public API

- FR-29: `NostrClient::new(config) -> Result<Self>`
- FR-30: `NostrClient::set_signer(signer) -> Result<()>`
- FR-31: `NostrClient::subscribe(filters) -> impl Stream<Item = Event>`
- FR-32: `NostrClient::publish(event) -> Result<EventId>`
- FR-33: `NostrClient::query(filters) -> impl Stream<Item = Event>`
- FR-34: `NostrClient::close()` for graceful shutdown

## Non-Goals

- **No GUI/Desktop app** - This is the core library only; UI bindings separate
- **No NIP-50 search** - Out of scope for initial migration
- **No NIP-65 relay management UI** - Core relay logic only
- **No WebTransport** - WebSocket only for now
- **No federation/gossip** - Direct relay connections only
- **No WASM bundling** - The core compiles to WASM but bundling is user's responsibility
- **No Swift/Kotlin wrappers** - UniFFI generates these; hand-written wrappers out of scope

## Design Considerations

### Worker Communication Pattern

Current WASM:
```
SAB Ring Buffer (single-producer/single-consumer)
┌─────────┐    ┌─────────┐
│ Worker  │◄──►│   SAB   │◄──►│ Worker  │
└─────────┘    └─────────┘     └─────────┘
```

New Native:
```
Tokio Channels (multi-producer/multi-consumer via clones)
┌─────────┐    ┌─────────┐
│ Worker  │◄──►│ Channel │◄──►│ Worker  │
└─────────┘    └─────────┘     └─────────┘
```

### Crate Structure

```
crates/
├── nipworker-core/          # Pure Rust, no platform deps
│   ├── src/
│   │   ├── lib.rs           # Public API
│   │   ├── workers/         # 4 workers
│   │   │   ├── mod.rs
│   │   │   ├── connections.rs
│   │   │   ├── cache.rs
│   │   │   ├── parser.rs
│   │   │   └── crypto.rs
│   │   ├── ipc/             # Channel definitions
│   │   ├── storage/         # EventStore trait + SQLite impl
│   │   ├── transport/       # WebSocketTransport trait
│   │   ├── protocol/        # FlatBuffers, Nostr types
│   │   └── signer/          # Signer traits
│   └── Cargo.toml
├── nipworker-server/        # HTTP/WebSocket server binary
│   └── src/main.rs
├── nipworker-ffi/           # UniFFI bindings
│   ├── src/lib.rs
│   └── nipworker.udl
└── nipworker-wasm/          # WASM-specific glue (optional)
```

### Feature Flags

```toml
[features]
default = ["tokio-runtime", "sqlite", "native-tls"]
tokio-runtime = ["tokio/full", "tokio-tungstenite"]
wasm-runtime = ["wasm-bindgen-futures", "gloo-net"]
sqlite = ["sqlx/sqlite", "sqlx/runtime-tokio"]
rocksdb = ["rocksdb"]  # Alternative storage
native-tls = ["tokio-tungstenite/native-tls"]
rustls = ["tokio-tungstenite/rustls-tls-webpki-roots"]
mobile = []  # Enables platform WebSocket trait usage
```

### Async Runtime Strategy

```rust
// Native: Tokio
#[cfg(not(target_arch = "wasm32"))]
pub use tokio::spawn as spawn_worker;

// WASM: wasm-bindgen-futures
#[cfg(target_arch = "wasm32")]
pub use wasm_bindgen_futures::spawn_local as spawn_worker;
```

## Technical Considerations

### Dependencies to Replace

| WASM Dependency | Native Replacement | Notes |
|-----------------|-------------------|-------|
| `js_sys::SharedArrayBuffer` | `tokio::sync::mpsc` | Different semantics but equivalent |
| `gloo-net::WebSocket` | `tokio-tungstenite` | Similar API |
| `gloo-timers` | `tokio::time` | Direct replacement |
| `web-sys::Idb*` | `sqlx::sqlite` | Schema migration needed |
| `wasm-bindgen-futures` | `tokio` | Feature-gated |
| `console_error_panic_hook` | `tracing-subscriber` | Better logging |

### Performance Targets

- Channel latency: <1ms (vs SAB rings)
- SQLite query: <10ms for 1000 events
- WebSocket connection: <500ms to first event
- Memory: <100MB baseline + cache size

### FFI Complexity

- UniFFI supports async/await (good!)
- Streams need to be converted to callbacks or channels
- Buffer management for binary data (FlatBuffers)
- Thread safety: core is `Send + Sync`, verify for FFI

## Success Metrics

- [ ] Core crate compiles on: Linux, macOS, Windows, iOS, Android, WASM
- [ ] All existing unit tests pass (ported to native)
- [ ] Integration test: connect to 5 relays, subscribe, receive 1000 events
- [ ] Memory usage within 10% of WASM version
- [ ] Throughput within 20% of WASM version
- [ ] Server binary starts in <2 seconds
- [ ] FFI bindings generate without warnings

## Open Questions

1. Should we use `tokio::task::spawn_blocking` for CPU-intensive crypto operations?
2. How to handle WebSocket certificate validation on mobile (platform certs vs rustls)?
3. Should we support WASM in the same crate or a separate `nipworker-wasm` crate?
4. What's the migration path for existing TypeScript users?
5. Do we need a migration guide for IndexedDB → SQLite data?
6. Should we keep the TypeScript orchestrator as an option, or fully replace?

---

## Implementation Phases

### Phase 1: Foundation (Weeks 1-2)
- Workspace structure
- IPC channels module
- Core types and traits

### Phase 2: Workers (Weeks 3-5)
- Crypto worker
- Parser worker
- Connections worker
- Cache worker

### Phase 3: Integration (Weeks 6-7)
- Orchestrator
- Public API
- Server binary

### Phase 4: FFI & Mobile (Weeks 8-9)
- UniFFI setup
- iOS bindings
- Android bindings

### Phase 5: WASM Compatibility (Week 10)
- Feature flags
- WASM CI
- Documentation
