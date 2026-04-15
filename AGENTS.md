# NIPWorker: Agent Guidelines

## Overview

NIPWorker is a high-performance Nostr client library using a multi-worker architecture with Rust WebAssembly. It implements 14+ NIPs with specialized workers for connections, caching, parsing, and cryptography.

## Prerequisites

- **Node.js**: 18+
- **Rust**: 1.70+
- **wasm-pack**: For building WASM modules (`curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh`)
- **flatc**: FlatBuffers compiler (for schema changes)

## Commands

### Build Commands
- **Build All**: `npm run build` - Builds all WASM crates + TypeScript bundle
- **Individual Crates**:
  - `npm run build:connections` - WebSocket connection worker
  - `npm run build:cache` - IndexedDB cache worker  
  - `npm run build:parser` - Event parsing and pipeline worker
  - `npm run build:crypto` - Cryptographic operations worker (signing, NIP-04/NIP-44 encryption)
- **Types**: `npm run build:types` - Generate .d.ts files via TypeScript

### Schema Commands
- **Generate FlatBuffers**: `npm run flatc` - Regenerate TS/Rust types from `/schemas`
  - `npm run flatc:rust` - Rust types only
  - `npm run flatc:ts` - TypeScript types only

### Release Commands
- **Release**: `./release.sh <version>` - Bump version, commit, tag, and push
- **Publish**: `npm run publish` - Build and publish to npm (runs automatically on git tags via CI)

## Code Style

- **TypeScript**: `camelCase` variables, `PascalCase` types. Explicit returns.
- **Formatting**: Tabs, 100w, single quotes (via `.prettierrc`).
- **Rust**: `snake_case`, `thiserror` for internals, `JSError` for WASM exports.
- **Imports**: External first, then internal project imports, then type-only.

## Architecture: 5 Rust Crates + TypeScript Orchestrator

### Crate Structure
```
src/
├── shared/          # Shared library (rlib) - MessagePort wrapper, telemetry
├── connections/     # WASM worker - WebSocket relay connections
├── cache/           # WASM worker - IndexedDB event caching
├── parser/          # WASM worker - Event parsing, validation, pipelines
└── crypto/          # WASM worker - Signing, encryption, proof verification
```

### Worker Communication
- **Orchestrator**: `NostrManager` (`src/index.ts`) spawns 4 Web Workers
- **IPC**: MessageChannel/MessagePort for cross-thread communication
- **Protocol**: FlatBuffers (binary) for serialized messages
- **Port Management**: Each worker pair has dedicated MessageChannels for bidirectional communication
- **Rust Port Wrapper**: `src/shared/src/port.rs` bridges JS MessagePort to Rust async mpsc channels

### Data Flow
```
App → NostrManager → parser worker → (cache/connections/crypto workers)
         ↓
    MessageChannel ports (via MessagePort API)
         ↓
    FlatBuffers serialized messages
```

### MessageChannel Topology
- `parser_cache`: parser ↔ cache
- `parser_connections`: parser ↔ connections
- `parser_crypto`: parser ↔ crypto
- `cache_connections`: cache ↔ connections
- `crypto_connections`: crypto ↔ connections
- `crypto_main`: crypto ↔ main thread
- `parser_main`: parser ↔ main (for batched events)
- `connections_main`: connections ↔ main (for relay status)

## Signer Types

The crypto worker supports multiple signer backends:
- **privkey**: Raw private key (hex)
- **nip07**: Browser extension (window.nostr)
- **nip46_bunker**: Remote signer via bunker URL
- **nip46_qr**: Remote signer via QR code / nostrconnect

## Key Files and Directories

| Path | Purpose |
|------|---------|
| `src/index.ts` | NostrManager - main orchestrator class |
| `src/hooks.ts` | React-style hooks (useSubscription, usePublish, useRelayStatus) |
| `src/utils.ts` | Utility functions, type guards, NIP-46 QR helper |
| `src/types/index.ts` | TypeScript type definitions |
| `src/lib/` | Shared library code (ArrayBufferReader, NostrUtils, NarrowTypes, etc.) |
| `src/proxy/` | WebSocket proxy server for environments needing relay connection proxying |
| `src/shared/src/port.rs` | MessagePort wrapper bridging JS to Rust async channels |
| `src/generated/` | FlatBuffers generated TypeScript code |
| `schemas/` | FlatBuffers schema definitions (.fbs files) |
| `release.sh` | Version bump and release automation |
| `.github/workflows/npm-publish.yml` | CI/CD for automated npm publishing |

## Package Exports

The library provides multiple entry points:
- `.` - Main NostrManager and types
- `./hooks` - React-style hooks
- `./utils` - Utility functions
- `./proxy` - Proxy client for relay connections
- `./proxy/server` - Proxy server implementation
- `./proxy/vite` - Vite plugin for proxy integration

## Zero-Copy Communication: What's True and What Isn't

### TypeScript → TypeScript Paths
**Fully zero-copy** via `postMessage(data, [transferable])`. Examples:
- `connections/proxy.ts` → `parserPort` / `cryptoPort` uses `[bytes.buffer]` transfer
- Main thread → worker init messages transfer MessagePorts directly

### Rust WASM Boundary
**Not zero-copy** in the absolute sense. WASM workers run in isolated linear memory, so data crossing the JS/WASM boundary must be copied at least once:

| Direction | Copies | Notes |
|-----------|--------|-------|
| **JS → Rust (receive)** | 1 | `ArrayBuffer`/`Uint8Array` is copied into `Vec<u8>` in `port.rs`. Unavoidable without SharedArrayBuffer because Rust needs `&[u8]` pointing to WASM linear memory. |
| **Rust → JS (send)** | 1 | Optimized to a single copy: Rust creates a standalone `ArrayBuffer`, copies bytes into it, then **transfers** the buffer via `post_message_with_transferable`. Before the optimization this was 2 copies. |

### FlatBuffers Parsing
FlatBuffers reads fields directly from the byte slice with **zero allocation** - no deserialization objects are created. This means the boundary copy is the primary overhead; the parsing itself is essentially free.

### SharedArrayBuffer: The Only True Zero-Copy Alternative
True zero-copy end-to-end would require `SharedArrayBuffer` (SAB) with all workers sharing the same memory buffer. This was the original architecture (`sab_ring.rs`), but it was replaced with MessageChannel for simpler synchronization and fewer COOP/COEP deployment requirements.

**Bottom line**: The current architecture is optimized but not strictly zero-copy across the WASM boundary. For the vast majority of Nostr workloads, the single-copy overhead is negligible compared to WebSocket I/O latency.

## Development Rules

### Schema Changes
1. Edit `.fbs` files in `/schemas` or `/schemas/kinds/`
2. Run `npm run flatc` immediately after modifications


### Build Order
1. `shared` is built as dependency of other crates (rlib)
2. `crypto` requires `shared` with `crypto` feature enabled
3. Other crates (parser, cache, connections) build in parallel

### Performance Guidelines
- Use `ArrayBufferReader` class for reading MessagePort binary data
- Favor cache over network (cacheFirst option)
- Set appropriate `bytesPerEvent` for subscriptions
- Use `closeOnEose: true` for one-time queries

### File Casing
- TS imports MUST match file casing exactly (e.g., `NostrUtils.ts`)
- This is enforced by `forceConsistentCasingInFileNames: true` in tsconfig

## CI/CD Pipeline

1. Push tag `v*` triggers GitHub Actions workflow
2. Workflow installs Rust, wasm-pack, wasm-opt (Binaryen)
3. Builds all WASM crates with release optimizations
4. Runs `npm run build` for TypeScript bundling
5. Publishes to npm (if version not already published)
6. Creates GitHub Release with auto-generated notes

## Dependencies

### Peer Dependencies (required by consumers)
- `flatbuffers: ^25.2.10`
- `vite: ^5.0.0 || ^6.0.0` (optional, for proxy/vite export)
- `ws: ^8.0.0` (optional, for proxy server)

### Runtime Dependencies
- `nostr-tools: ^2.0.0`
- `socks-proxy-agent: ^9.0.0`

### Dev Dependencies
- Vite with plugins: wasm, top-level-await, dts, static-copy
- TypeScript 5.x, Prettier 3.x

## NIP Support

Implemented NIPs with their event kinds:
- NIP-01 (kinds 0, 1): Basic protocol
- NIP-02 (kind 3): Contact list
- NIP-04 (kind 4): Encrypted DMs
- NIP-18 (kind 6): Reposts
- NIP-19: bech32 entities
- NIP-25 (kind 7): Reactions
- NIP-44: Versioned encryption
- NIP-51 (kind 39089): Categorized lists
- NIP-57 (kind 9735): Lightning zaps
- NIP-60 (kinds 7374, 7375, 7376, 10019, 17375): Cashu wallet
- NIP-61 (kind 9321): Nutzaps
- NIP-65 (kind 10002): Relay lists
