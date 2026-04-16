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
