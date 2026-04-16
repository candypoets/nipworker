# Mission Status: nipworker/phase-a-core-extraction

## Init
- **Started:** 2026-04-16
- **Branch:** refactor
- **Base commit range:** 8527729..72e33ed (Milestones 1-9 already committed)
- **Plan approved and mission started.**

## Milestone 0: Retrospective Review
- **Status:** DONE
- **Commit:** 70d88ec3724bcc0b3b2ed7e919f1fc5379634b43
- **Findings:** Codex review found 4 critical + 3 high-severity issues. Review agent applied fixes:
  - Replaced `std::thread::sleep` busy-wait in `crates/core/src/network/subscription.rs` with `async_lock::Semaphore`
  - Replaced blocking `std::sync::mpsc::recv()` in `crates/native-ffi/src/lib.rs` with `tokio::sync::mpsc::unbounded_channel`
  - Fixed native C callback UAF by leaking payload Vec + adding `nipworker_free_bytes()`
  - Fixed storage key collision (`event_bytes.len()` → `hex::encode(event_bytes)`) in native-ffi and engine
  - Fixed `src/EngineManager.ts` message routing to use transferred MessagePort
  - Fixed `src/engine/src/lib.rs` to prepend 4-byte LE length to event bytes for `ArrayBufferReader`
- **Build status:** All crates compile. Pre-existing TS errors documented.

## Milestone 10: Engine Signer Setup & SignEvent
- **Status:** DONE
- **Commit:** 772f10a5d696e482418c5b6f35ecbb8bc638a137
- **Changes:**
  - `NostrEngine` signer changed to `Arc<RwLock<Arc<dyn Signer>>>` with `set_signer` method
  - `SetSigner` and `SignEvent` branches fully wired in `handle_message`
  - `PrivateKeySigner` trait implementation added
  - WASM `setPrivateKey` wired to underlying signer
  - `EngineManager` missing methods added (`setNip46Bunker`, `setNip46QR`, `setNip07`, `setPubkey`)
- **Review fixes:**
  - EngineManager now sends FlatBuffers `SetSigner`/`SignEvent` instead of legacy JSON
  - EngineManager onmessage routes `Pubkey` and `SignedEvent` responses
  - Removed unnecessary `SignerArcWrapper` abstraction

## Milestone 11: WASM IndexedDB Storage
- **Status:** DONE
- **Commit:** 051a6a2
- **Changes:**
  - Added `IndexedDbStorage` in `src/engine/src/storage.rs` backed by browser IndexedDB
  - Added `web-sys` IDB features to `src/engine/Cargo.toml`
  - Swapped `MemoryStorage` for `IndexedDbStorage` in `src/engine/src/lib.rs`
- **Review fix:** Replaced `window()` with `js_sys::global()` + `Reflect::get` for Web Worker compatibility

## Milestone 12: Engine NIP-07 & NIP-46 Signers
- **Status:** DONE
- **Commit:** 252403e
- **Changes:**
  - Added `ProxySigner` in `src/engine/src/signer.rs` implementing `Signer` trait via main-thread proxy
  - Wired `set_proxy_signer` and `signer_response` in `src/engine/src/lib.rs`
  - Updated `EngineManager` to proxy NIP-07 to `window.nostr` and NIP-46 to `nostr-tools` `BunkerSigner`
  - Added generation counters to prevent stale async NIP-46 init races

## Milestone 13: Native FFI Hardening
- **Status:** DONE
- **Commit:** d303648b88c9a5f4abba5888292979138409f2c9
- **Changes:**
  - Added `AtomicBool` destruction flag and handle UAF protection in `crates/native-ffi/src/lib.rs`
  - Fixed `NativeTransport::disconnect` to properly close WebSocket reader task via `tokio::select!`
  - Replaced `Mutex::lock().unwrap()` with poison recovery in `crates/native-ffi/src/signer.rs`
  - Added `macros` feature to `tokio` in `Cargo.toml`
- **Review fixes:** Replaced separate `AtomicBool` + sender with unified `Mutex<NipworkerState>` to prevent teardown races while still leaking the Box for UAF safety
