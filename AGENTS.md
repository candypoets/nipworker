# NIPWorker: Agent Guidelines

## Overview

NIPWorker is a high-performance Nostr client library using a multi-worker architecture with Rust WebAssembly. It implements 14+ NIPs with specialized workers for connections, caching, parsing, and cryptography. Native targets (iOS, Android, React Native) reuse the same Rust core through a C FFI instead of WASM workers.

## Prerequisites

- **Node.js**: 18+
- **Rust**: 1.70+
- **wasm-pack**: For building WASM modules (`curl https://rustwasm.github.io/wasm-pack/installer/init.sh -sSf | sh`)
- **flatc**: FlatBuffers compiler (for schema changes)
- **Xcode / Android SDK+NDK**: Only for native mobile builds (`build:native:*`)

## Commands

### Build Commands
- **Build All**: `npm run build` - Builds all WASM crates + native Android/iOS artifacts + TypeScript bundle
- **WASM Crates Only**: `npm run build:crates` - Builds the 4 WASM crates in parallel
- **Individual WASM Crates**:
  - `npm run build:connections` - WebSocket connection worker
  - `npm run build:cache` - IndexedDB cache worker  
  - `npm run build:parser` - Event parsing and pipeline worker
  - `npm run build:crypto` - Cryptographic operations worker (signing, NIP-04/NIP-44 encryption)
- **Native Builds** (crates/native-ffi):
  - `npm run build:native` - Android AAR + iOS XCFramework in parallel
  - `npm run build:native:android` - Android AAR (`crates/native-ffi/android`)
  - `npm run build:native:ios` - iOS XCFramework (`crates/native-ffi/ios`)
- **Types**: `npm run build:types` - Generate .d.ts files via TypeScript

### Schema Commands
- **Generate FlatBuffers**: `npm run flatc` - Regenerate Rust/TS/Java types from `/schemas`
  - `npm run flatc:rust` - Rust types only (`crates/core/src/generated`)
  - `npm run flatc:ts` - TypeScript types only (`src/generated`)
  - `npm run flatc:java` - Java types only (React Native Android)
  - `npm run flatc:swift` - Swift types + `swift build` (run separately; not part of `flatc`)

### Test Commands
- **Unit Tests**: `npm test` - Vitest
- **E2E Tests**: `npm run test:e2e` - Playwright browser tests
- **Benchmarks**: `npm run bench` - Rust criterion micro-benchmarks (crates/core); `npm run bench:browser` - Playwright end-to-end bench suite (mock relay, self-starting). Baselines and findings in `BENCHMARKS.md`.

### Release Commands
- **Release**: `./release.sh <version>` - Bump version, commit, tag, and push
- **Publish**: `npm run publish` - Build and publish to npm (runs automatically on git tags via CI)

## Code Style

- **TypeScript**: `camelCase` variables, `PascalCase` types. Explicit returns.
- **Formatting**: Tabs, 100w, single quotes (via `.prettierrc`).
- **Rust**: `snake_case`, `thiserror`-based `NostrError` (`crates/core/src/nostr_error.rs`) for internals. WASM exports return `()` and report via `tracing` (initialized with `init_tracing`), not `JSError`.
- **Imports**: External first, then internal project imports, then type-only.

## Architecture: crates/core + Thin Shells + TypeScript Orchestrator

All business logic lives in a single Rust library (`crates/core`, rlib) with feature-gated modules. Thin `cdylib` shells wrap it for each WASM worker, and a native FFI crate exposes it to mobile platforms.

### Crate Structure
```
crates/
├── core/          # rlib - all logic, feature-gated modules: parser, cache, connections, crypto
│                  #   channel.rs        - WorkerChannel/MessageSender abstraction
│                  #   service/engine.rs - NostrEngine (wires all workers in one process)
│                  #   worker/           - parser/cache/connections/crypto worker implementations
├── parser/        # cdylib shell - WASM parser worker
├── connections/   # cdylib shell - WASM WebSocket connection worker
├── cache/         # cdylib shell - WASM IndexedDB cache worker
├── crypto/        # cdylib shell - WASM signing/encryption worker
├── mesh/          # rlib - BLE/NIP-77 negentropy mesh sessions (native only)
└── native-ffi/    # cdylib+staticlib - C ABI for iOS/Android/React Native,
                   #   runs NostrEngine on OS threads (no WASM)
```

### Worker Communication
- **Orchestrator**: `NostrManager` (`src/NostrManager.ts`) spawns 4 Web Workers; `createNostrManager()` (`src/index.ts`) always returns this 4-worker backend
- **IPC**: MessageChannel/MessagePort for cross-thread communication
- **Protocol**: FlatBuffers (binary) for serialized messages
- **Port Management**: Each worker pair has dedicated MessageChannels for bidirectional communication
- **Rust Channel Wrapper**: `crates/core/src/channel.rs` defines the `WorkerChannel`/`MessageSender` traits; `WasmWorkerChannel` bridges a JS MessagePort to Rust async channels (native builds use in-process futures/tokio channel implementations of the same traits)

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
- `parser_main`: parser ↔ main (for batched events and relay status)
- `mesh_cache`: mesh ↔ cache (native mesh builds only; wired in-process inside `NostrEngine` so a mesh cache miss never escapes to WebSocket connections)

## Signer Types

The crypto worker supports multiple signer backends:
- **privkey**: Raw private key (hex)
- **nip07**: Browser extension (window.nostr)
- **nip46_bunker**: Remote signer via bunker URL
- **nip46_qr**: Remote signer via QR code / nostrconnect

## Key Files and Directories

| Path | Purpose |
|------|---------|
| `src/index.ts` | Package entry; `createNostrManager()` factory |
| `src/NostrManager.ts` | NostrManager - main orchestrator class (4 WASM workers) |
| `src/hooks.ts` | React-style hooks (useSubscription, usePublish, useRelayStatus) |
| `src/utils.ts` | Utility functions, type guards, NIP-46 QR helper |
| `src/types/index.ts` | TypeScript type definitions |
| `src/lib/` | Shared library code (ArrayBufferReader, NostrUtils, NarrowTypes, etc.) |
| `src/proxy/` | WebSocket proxy server for environments needing relay connection proxying |
| `crates/core/` | Shared Rust library (rlib) with all worker logic |
| `crates/core/src/channel.rs` | WorkerChannel/MessageSender traits; WasmWorkerChannel MessagePort bridge |
| `crates/core/src/service/engine.rs` | NostrEngine - all workers wired in one process (used by native-ffi) |
| `crates/mesh/` | BLE/NIP-77 negentropy mesh sessions (native only) |
| `crates/native-ffi/` | C ABI for iOS/Android/React Native |
| `swift/` | SwiftPM package (NipworkerSwift) wrapping native-ffi for Apple platforms |
| `src/generated/` | FlatBuffers generated TypeScript code |
| `schemas/` | FlatBuffers schema definitions (.fbs files) |
| `release.sh` | Version bump and release automation |
| `.github/workflows/npm-publish.yml` | CI/CD for automated npm publishing |
| `.github/workflows/native-build.yml` | CI builds of Android/iOS native artifacts |

## Package Exports

The library provides multiple entry points:
- `.` - Main NostrManager and types
- `./hooks` - React-style hooks
- `./utils` - Utility functions
- `./proxy` - Proxy client for relay connections
- `./proxy/server` - Proxy server implementation
- `./proxy/vite` - Vite plugin for proxy integration
- `./react-native` - React Native entry (TurboModule via native-ffi, no WASM)
- `./legacy` - Alias for the main 4-worker NostrManager (legacy import path)

## Zero-Copy Communication: What's True and What Isn't

### TypeScript → TypeScript Paths
**Fully zero-copy** via `postMessage(data, [transferable])`. Examples:
- `connections/proxy.ts` → `parserPort` / `cryptoPort` uses `[bytes.buffer]` transfer
- Main thread → worker init messages transfer MessagePorts directly

### Rust WASM Boundary
**Not zero-copy** in the absolute sense. WASM workers run in isolated linear memory, so data crossing the JS/WASM boundary must be copied at least once:

| Direction | Copies | Notes |
|-----------|--------|-------|
| **JS → Rust (receive)** | 1 | `ArrayBuffer`/`Uint8Array` is copied into `Vec<u8>` in `WasmWorkerChannel` (`crates/core/src/channel.rs`). Unavoidable without SharedArrayBuffer because Rust needs `&[u8]` pointing to WASM linear memory. |
| **Rust → JS (send)** | 1 | Optimized to a single copy: Rust creates a standalone `ArrayBuffer`, copies bytes into it, then **transfers** the buffer via `post_message_with_transferable`. Before the optimization this was 2 copies. |

### FlatBuffers Parsing
FlatBuffers reads fields directly from the byte slice with **zero allocation** - no deserialization objects are created. This means the boundary copy is the primary overhead; the parsing itself is essentially free.

### SharedArrayBuffer: The Only True Zero-Copy Alternative
True zero-copy end-to-end would require `SharedArrayBuffer` (SAB) with all workers sharing the same memory buffer. This was the original architecture (`sab_ring.rs`), but it was replaced with MessageChannel for simpler synchronization and fewer COOP/COEP deployment requirements.

**Bottom line**: The current architecture is optimized but not strictly zero-copy across the WASM boundary. For the vast majority of Nostr workloads, the single-copy overhead is negligible compared to WebSocket I/O latency. Native (non-WASM) builds skip the boundary entirely - channels move `Vec<u8>` between threads in-process.

## Development Rules

### Schema Changes
1. Edit `.fbs` files in `/schemas` or `/schemas/kinds/`
2. Run `npm run flatc` immediately after modifications (Rust/TS/Java)
3. Run `npm run flatc:swift` as well if the Swift package consumes the changed schema


### Build Order
1. `crates/core` (rlib) is the single source of logic; each cdylib shell compiles it with its matching feature enabled (`parser`, `cache`, `connections`, or `crypto`)
2. The `crypto` feature pulls the heavy crypto deps (k256, sha2, chacha20, aes, hkdf, ...)
3. WASM shells (`crates/{parser,connections,cache,crypto}`) build in parallel via wasm-pack
4. `crates/mesh` and `crates/native-ffi` are native-only - they never build to WASM

### Performance Guidelines
- Use `ArrayBufferReader` class for reading MessagePort binary data
- Favor cache over network (cacheFirst option)
- Set appropriate `bytesPerEvent` for subscriptions
- Use `closeOnEose: true` for one-time queries
- `Eoce` (end-of-cached-events) is a distinct message emitted by the cache worker when cached results are exhausted - it is NOT a typo for relay `EOSE`

### File Casing
- TS imports MUST match file casing exactly (e.g., `NostrUtils.ts`)
- This is enforced by `forceConsistentCasingInFileNames: true` in tsconfig

## CI/CD Pipeline

1. Push tag `v*` triggers the npm-publish and native-build GitHub Actions workflows
2. `native-build.yml` builds the Android AAR and iOS XCFramework artifacts
3. `npm-publish.yml` installs Rust, wasm-pack, wasm-opt (Binaryen), waits for the native artifacts, and builds all WASM crates with release optimizations
4. Runs `npm run build` for TypeScript bundling
5. Publishes to npm (if version not already published)
6. Creates GitHub Release with auto-generated notes

## Dependencies

### Peer Dependencies (required by consumers)
- `flatbuffers: ^25.2.10`
- `react-native: >=0.72` (optional, for the react-native export)
- `vite: ^5.0.0 || ^6.0.0` (optional, for proxy/vite export)
- `ws: ^8.0.0` (optional, for proxy server)

### Runtime Dependencies
- `@babel/runtime: ^7.29.7`
- `nostr-tools: ^2.0.0`
- `socks-proxy-agent: ^9.0.0`
- `ws: ^8.0.0`

### Dev Dependencies
- Vite with plugins: wasm, top-level-await, dts, static-copy
- TypeScript 5.x, Prettier 3.x, Vitest 4.x, Playwright

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
