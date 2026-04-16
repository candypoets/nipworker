# Phase A: Core Extraction + WASM Re-platforming

## Mission Goal
Expose nipworker in three forms:
1. **Current NPM (WASM+Workers)** — backward compatible
2. **Rust crate** (`nipworker-core`) — for Rust consumers
3. **Native FFI** — for LynxJS / React Native / other native apps

## How We Got Here

### Milestone 1: Bootstrap crates/core and move shared types
- Created `crates/core/` as the platform-agnostic Rust core
- Moved FlatBuffers generated code, shared types (nostr, network, proof), and crypto utilities from `src/shared/`
- Defined the three core traits: `Transport`, `Storage`, `Signer` using `#[async_trait(?Send)]` for WASM compatibility

### Milestones 3-6: Move business logic into crates/core
- **Parser**: All kind-specific parsers, content parsing, pipeline infrastructure
- **Crypto**: All signers (PrivateKey, NIP-04, NIP-44, NIP-46), cryptographic utilities
- **Connections**: Connection registry and WebSocket management (gated with `#[cfg(target_arch = "wasm32")]`)
- **Cache**: Storage abstraction and in-memory implementations

### Milestone 6.5: Fix core compilation
- Fixed import paths, FlatBuffers schema mismatches, and trait method signatures
- `cargo check --features crypto` passes in `crates/core/`

### Milestone 7a: Build NostrEngine
- Created `NostrEngine` in `crates/core/src/service/engine.rs`
- Wires together `Transport`, `Storage`, `Signer`, `Parser`, `Pipeline`, `NetworkManager` internally
- Uses `futures::channel::mpsc` for internal async communication
- Exposes `handle_message(&self, bytes: &[u8])` for FlatBuffers command dispatch

### Milestone 7b: Create WASM engine wrapper
- Created `src/engine/` as a single-worker WASM crate (`nipworker-engine`)
- Implements `WebSocketTransport` using `gloo_net::websocket`
- Implements `InMemoryStorage` using `async-lock::RwLock`
- Implements `LocalSigner` wrapping `PrivateKeySigner`
- Compiles for `wasm32-unknown-unknown`

### Milestone 8: Update TypeScript layer
- Created `EngineManager` as drop-in replacement for `NostrManager`
- Updated `createNostrManager(config)` to accept `{ engine: true }` to use new backend
- Added `src/engine/index.ts` as worker entry point
- Updated `package.json` build scripts to include `build:engine`

### Milestone 9: Fix Send/Sync and native-ffi
- Reverted core traits to `#[async_trait(?Send)]` to keep WASM working (WASM futures are not `Send`)
- Updated WASM engine to use `?Send`
- Created `crates/native-ffi/` with C ABI exports:
  - `nipworker_init(callback)` — returns opaque handle, spawns background thread with `tokio::task::LocalSet`
  - `nipworker_handle_message(handle, ptr, len)` — queues FlatBuffers command
  - `nipworker_set_private_key(handle, c_str)` — queues signer setup
  - `nipworker_deinit(handle)` — cleanup
- Uses single-threaded `LocalSet` executor on dedicated native thread (off main thread)

## Current Architecture

```
┌─────────────────────────────────────────────────────────────────────────┐
│                              JS Consumer                                │
│  ┌─────────────────┐    ┌──────────────────────────────────────────┐   │
│  │  NostrManager   │◄──►│  Pluggable Backend Interface             │   │
│  │  (4 workers)    │    │  - WorkerBackend  (WASM + MessageChannel)│   │
│  └─────────────────┘    │  - EngineBackend  (WASM + Single Worker)   │   │
│  ┌─────────────────┐    │  - NativeBackend  (Lynx Native Module)   │   │
│  │  EngineManager  │◄───┘                                            │   │
│  │  (1 worker)     │                                                 │   │
│  └─────────────────┘                                                 │   │
└─────────────────────────────────────────────────────────────────────────┘
                                    │
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           ▼
┌───────────────┐           ┌───────────────┐           ┌───────────────┐
│  src/engine   │           │ crates/core   │           │crates/native-ffi
│  (WASM)       │◄─────────►│  (pure Rust)  │◄─────────►│  (C ABI)      │
│               │           │               │           │               │
│ WebSocketTransport          │ NostrEngine   │           │NativeTransport│
│ MemoryStorage               │               │           │InMemoryStorage│
│ LocalSigner                 │               │           │ NativeSigner  │
└───────────────┘           └───────────────┘           └───────────────┘
                                                               │
                                                    ┌──────────┼──────────┐
                                                    ▼          ▼          ▼
                                              ┌─────────┐ ┌─────────┐ ┌─────────┐
                                              │  iOS    │ │ Android │ │ Harmony │
                                              │ ObjC++  │ │  Kotlin │ │  ArkTS  │
                                              │ Module  │ │  Module │ │  Module │
                                              └─────────┘ └─────────┘ └─────────┘
```

## What Works Now

| Component | Compiles | Notes |
|-----------|----------|-------|
| `crates/core` | ✅ | `cargo check --features crypto` |
| `src/engine` (WASM) | ✅ | `cargo check --target wasm32-unknown-unknown` |
| `crates/native-ffi` | ✅ | `cargo check` |
| 4-worker original | ✅ | Untouched, still default |
| TypeScript layer | ✅ | `npx tsc --noEmit` passes |

## Known Gaps / TODO for Next Phase

1. **Engine Signer Setter**: `NostrEngine` doesn't expose signer setup after construction; `nipworker_set_private_key` queues it but engine doesn't process yet
2. **Engine IndexedDB Storage**: Currently in-memory only; needs IndexedDB trait impl for persistence
3. **Engine NIP-07**: Browser extension signer not wired in single-worker mode
4. **Engine NIP-46**: Remote signer not wired in single-worker mode
5. **Native FFI Callback Safety**: C callback needs proper lifetime management (currently synchronous call from Rust thread)
6. **Lynx Native Modules**: iOS/Android/Harmony wrappers not yet written

## How to Continue

### Option A: Finish Engine (web-first)
- Add signer setup command processing to `NostrEngine::handle_message`
- Implement IndexedDB-backed `Storage` for WASM
- Wire NIP-07 and NIP-46 for single-worker mode
- Benchmark single vs 4-worker performance

### Option B: Go Native (Lynx)
- Write iOS `LynxModule` in Objective-C++ that links `libnipworker_native_ffi.a`
- Write Android `LynxModule` in Kotlin that loads `libnipworker_native_ffi.so`
- Write HarmonyOS `LynxModule` in ArkTS
- Create TS `LynxNativeBackend` that calls `NativeModules.Nipworker`

### Option C: Re-platform 4 Workers
- Refactor `src/connections`, `src/cache`, `src/parser`, `src/crypto` to use `nipworker-core` internally
- Keep 4-worker topology but delegate to shared core services
- Removes code duplication between old and new paths

## Commit History for This Mission

```
72e33ed Milestone 9: Fix native-ffi and WASM engine compilation after Send/Sync refactor
e0bde09 Milestone 8: Update TS layer with EngineManager, engine worker entry point, and build integration
3cbf466 Milestone 7b: Create WASM engine wrapper crate (nipworker-engine)
349d8eb Milestone 7a: Build NostrEngine in core with trait-based Transport, Storage, Signer
dbed46a Milestone 6.5: Fix core compilation errors
64572da Milestones 3-6: Move parser, crypto, connections, and cache logic into crates/core
8527729 Milestone 1: Bootstrap crates/core and move shared types
```
