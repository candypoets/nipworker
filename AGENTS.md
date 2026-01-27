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
  - `npm run flatc:ts` - TypeScript types only (runs patch-strings.js after)

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
├── shared/          # Shared library (rlib) - SAB ring buffers, telemetry
├── connections/     # WASM worker - WebSocket relay connections
├── cache/           # WASM worker - IndexedDB event caching
├── parser/          # WASM worker - Event parsing, validation, pipelines
└── crypto/          # WASM worker - Signing, encryption, proof verification
```

### Worker Communication
- **Orchestrator**: `NostrManager` (`src/index.ts`) spawns 4 Web Workers
- **IPC**: SharedArrayBuffer (SAB) ring buffers using `sab_ring.rs` (`src/shared/src/`)
- **Protocol**: FlatBuffers (binary) for zero-copy cross-thread communication
- **Ring Buffers**: Each worker pair has dedicated SABs for request/response
- **Header Layout**: 32-byte header (capacity, head, tail, seq, reserved)

### Data Flow
```
App → NostrManager → parser worker → (cache/connections/crypto workers)
         ↓
    SharedArrayBuffer rings (via sab_ring.rs)
         ↓
    FlatBuffers serialized messages
```

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
| `src/lib/` | Shared library code (ByteString, SharedBuffer, NostrUtils, etc.) |
| `src/ws/` | WebSocket runtime and connection management |
| `src/generated/` | FlatBuffers generated TypeScript code |
| `schemas/` | FlatBuffers schema definitions (.fbs files) |
| `scripts/patch-strings.js` | Post-processing for FlatBuffers TS output |
| `release.sh` | Version bump and release automation |
| `.github/workflows/npm-publish.yml` | CI/CD for automated npm publishing |

## Development Rules

### Schema Changes
1. Edit `.fbs` files in `/schemas` or `/schemas/kinds/`
2. Run `npm run flatc` immediately after modifications
3. The patch-strings.js script auto-runs to fix ByteString imports

### IPC Safety
- Header layout in `sab_ring.rs` must remain 32-byte consistent
- Never change ring buffer header structure without updating all workers

### Build Order
1. `shared` is built as dependency of other crates (rlib)
2. `crypto` requires `shared` with `crypto` feature enabled
3. Other crates (parser, cache, connections) build in parallel

### Performance Guidelines
- Use `SharedBufferReader` class for reading from SABs
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

### Runtime Dependencies
- `nostr-tools: ^2.0.0`

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
