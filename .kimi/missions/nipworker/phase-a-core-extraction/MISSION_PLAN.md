# Mission Plan: nipworker/phase-a-core-extraction

## Context

Phase A (Core Extraction + WASM Re-platforming) was executed manually across 9 milestones without the mission skill framework. The work is committed on the `refactor` branch (commits `8527729` through `72e33ed`). All three target forms are bootstrapped and compile:

1. **Current NPM (WASM+Workers)** — untouched 4-worker backend, still default
2. **Rust crate (`nipworker-core`)** — `cargo check --features crypto` passes
3. **Native FFI** — `cargo check` passes

However, multiple stubs and known gaps remain. This mission formally completes Phase A by retrospectively reviewing the existing work, then implementing all remaining stubs through the mission skill protocol (feature agents → validation → Codex review → commit).

## Milestones

### Milestone 0: Retrospective Review of Committed Work
**Objective:** Run Codex review on the aggregate Phase A diff (M1–M9) and fix any critical architectural or safety issues before building on top of it.

**Success Criteria:**
- Codex review completes with no critical findings, OR all critical findings are fixed and re-reviewed.
- All existing crates still compile (`crates/core`, `src/engine`, `crates/native-ffi`, original workers).
- `npx tsc --noEmit` passes (or pre-existing errors are documented).

**Features:**
1. Create a temporary review commit spanning the Phase A diff.
2. Run Codex CLI review (`codex -m gpt-5.4 -c reasoning_effort="xhigh" review`) on the aggregate diff.
3. Spawn fix agents for any critical bugs or safety issues found.
4. Re-run review if fixes were applied.

---

### Milestone 10: Engine Signer Setup & SignEvent
**Objective:** Make `NostrEngine` support runtime signer replacement and fully wired `SignEvent` / `GetPublicKey` handling over FlatBuffers.

**Success Criteria:**
- `SetSigner` FlatBuffers message is processed (not a no-op stub).
- `SignEvent` FlatBuffers message builds a signed event and returns it via the event sink.
- WASM engine `setPrivateKey` call actually updates the signer.
- `cargo check --target wasm32-unknown-unknown` in `src/engine` passes.

**Features:**
1. Replace `signer: Arc<dyn Signer>` in `NostrEngine` with `Arc<RwLock<dyn Signer>>` (or `async-lock::RwLock` for WASM) and implement `set_signer`.
2. Wire `SetSigner` branch in `NostrEngine::handle_message` to call `set_signer`.
3. Implement `SignEvent` branch: parse `Template`, call `signer.sign_event`, build `WorkerMessage` with `SignedEvent`, send via event sink.
4. Update `src/engine/src/lib.rs` `setPrivateKey` to acquire the signer lock and call `set_private_key`.
5. Update `src/engine/src/signer.rs` `LocalSigner` to support being updated after construction.
6. Fix any TypeScript type errors in `EngineManager` related to signer methods.

---

### Milestone 11: WASM IndexedDB Storage
**Objective:** Replace the in-memory `MemoryStorage` in the WASM engine with a persistent IndexedDB-backed implementation.

**Success Criteria:**
- `src/engine` compiles for `wasm32-unknown-unknown` with the new storage.
- IndexedDB storage implements the `nipworker_core::traits::Storage` trait.
- Events persisted survive a page reload (verified via a light integration check).

**Features:**
1. Add `web-sys` IndexedDB bindings (`IdbFactory`, `IdbDatabase`, `IdbObjectStore`, etc.) to `src/engine/Cargo.toml`.
2. Implement `IndexedDbStorage` in `src/engine/src/storage.rs` with `initialize`, `persist`, and `query`.
3. Use a simple object store keyed by event id (computed from bytes), storing raw event bytes.
4. For `query`, implement basic tag/time filtering or fallback to loading-all-and-filtering in-memory (acceptable for first iteration).
5. Swap `MemoryStorage` for `IndexedDbStorage` in `src/engine/src/lib.rs`.
6. Run `cargo check --target wasm32-unknown-unknown` in `src/engine`.

---

### Milestone 12: Engine NIP-07 & NIP-46 Signers
**Objective:** Wire browser-extension (NIP-07) and remote (NIP-46) signers into the single-worker engine backend.

**Success Criteria:**
- `EngineManager.setSigner('nip07', ...)` no longer warns and works in engine mode.
- `EngineManager.setSigner('nip46', ...)` no longer warns and works in engine mode.
- The WASM engine can request signing/encryption from the main thread and receive responses.

**Features:**
1. Extend the FlatBuffers `SignerType` union (or add a `Proxy` variant) to support a main-thread-proxy signer. *Decision:* if schema changes are too invasive, use a string-based `signer_type` field or a dedicated `ProxySigner` table.
2. Implement `ProxySigner` in `crates/core/src/crypto/signers/` (or `src/engine/src/signer.rs`) that implements the `Signer` trait by sending `SignerRequest` messages over an async channel to the JS side.
3. Add a request/response correlation map in `EngineManager` to match signer responses.
4. Wire `EngineManager` NIP-07 path: receive proxy sign requests, call `window.nostr`, return results to the WASM engine.
5. Wire `EngineManager` NIP-46 path: either (a) reuse existing `src/crypto` worker logic in the main thread, or (b) implement a lightweight NIP-46 client in TS that proxies crypto ops back to the engine. Choose (a) if feasible to minimize new code.
6. Update `setSigner` in `EngineManager` to handle `nip07`/`nip46` in engine mode.
7. Compile and type-check TypeScript layer.

---

### Milestone 13: Native FFI Hardening
**Objective:** Fix safety gaps in `crates/native-ffi` so it is production-ready for Lynx/React Native integration.

**Success Criteria:**
- No use-after-free possible on the C handle.
- C callback contract is documented and safe.
- WebSocket disconnect closes the connection cleanly.
- Storage key collision is fixed.
- `cargo check` in `crates/native-ffi` passes.

**Features:**
1. Replace raw handle pointer with an `Arc<AtomicBool>` or ref-counted struct that tracks "destroyed" state; guard all handle methods.
2. Document the callback contract: pointer is valid only for the duration of the call, C side must copy immediately.
3. Fix `NativeTransport::disconnect` to close the WebSocket stream (signal writer/reader tasks to stop).
4. Fix `InMemoryStorage::persist` to use the event id (SHA-256 hash of serialized event) as the HashMap key instead of byte length.
5. Replace `Mutex::lock().unwrap()` with `Mutex::lock().unwrap_or_else(|e| e.into_inner())` or `parking_lot::Mutex` to avoid poison panics.
6. Clean up dead code/unused channels in `crates/native-ffi/src/lib.rs`.

---

### Milestone 14: Lynx Native Modules
**Objective:** Provide initial platform module skeletons so native mobile apps can consume `libnipworker_native_ffi`.

**Success Criteria:**
- iOS, Android, and HarmonyOS module stubs exist and compile conceptually.
- TypeScript `NativeBackend` exists and exposes the same interface as `EngineManager`/`NostrManager`.
- No breaking changes to existing exports.

**Features:**
1. Create `crates/native-ffi/ios/LynxNipworkerModule.mm` — Objective-C++ Lynx module that loads `libnipworker_native_ffi.a`, initializes the engine, and forwards FlatBuffers messages.
2. Create `crates/native-ffi/android/LynxNipworkerModule.kt` — Kotlin Lynx module that loads `libnipworker_native_ffi.so` with the same interface.
3. Create `crates/native-ffi/harmony/LynxNipworkerModule.ets` — ArkTS Lynx module stub for HarmonyOS.
4. Create `src/NativeBackend.ts` implementing `subscribe`, `unsubscribe`, `publish`, `setSigner`, etc. by calling the platform native module.
5. Export `NativeBackend` from `src/index.ts`.
6. Document known untested status of the native modules.

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| Schema change for `SignerType` breaks 4-worker backend | Keep `SignerType` changes additive; run flatc and verify original workers still compile. |
| IndexedDB in Rust via web-sys is complex | Start with simple object-store-per-event design; accept in-memory filtering for query. |
| NIP-46 in single worker may need its own WebSocket | If proxying through main thread is too complex, instantiate core NIP-46 signer directly in WASM engine with `gloo_net::websocket`. |
| Native modules cannot be tested in this environment | Scope tightly to skeletons; mark as untested in docs. |
| Send/Sync regressions when modifying core traits | Keep `#[async_trait(?Send)]`; do not add `Send` bounds. |

## Estimated Runs

- M0 retrospective: 1 review + up to 1 fix loop = ~2 runs
- M10 signer: 2 features + 1 validation + 1 review = ~4 runs
- M11 indexeddb: 2 features + 1 validation + 1 review = ~4 runs
- M12 nip07/46: 3 features + 1 validation + 1 review = ~5 runs
- M13 native-ffi: 2 features + 1 validation + 1 review = ~4 runs
- M14 lynx: 2 features + 1 validation + 1 review = ~4 runs

**Total estimated runs: ~23**
