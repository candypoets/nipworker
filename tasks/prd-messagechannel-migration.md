# PRD: MessageChannel Worker Communication Migration

## Introduction

Replace the SharedArrayBuffer (SAB) ring buffer IPC between Web Workers with MessageChannel ports. This eliminates the complex polling logic, Spectre attack surface, and SRSP (Single Reader Single Producer) limitations while maintaining high performance through native browser message passing.

**Key Improvement:** Workers use `select!` to await messages from multiple sources instead of polling SAB rings with exponential backoff.

## Goals

- Eliminate all SAB-based worker-to-worker communication
- Replace polling loops with async message reception via `select!`
- Maintain existing FlatBuffer message schemas (zero changes)
- Preserve existing sharding architecture in parser
- Simplify code by merging `db_ring` into `parser→cache` channel
- Achieve cleaner 1:1 channel relationships between worker pairs

## Non-Goals (Out of Scope)

- Main thread communication (keep existing SAB for subscriptions)
- Status ring (keep SAB for now)
- Changes to FlatBuffer schemas
- Changes to worker sharding logic
- Changes to IndexedDB operations
- NIP-07 extension handling (stays postMessage)

---

## User Stories

### US-001: Create Port wrapper module in shared crate

**Description:** As a developer, I need a thin wrapper around MessagePort to enable async message reception in Rust workers.

**Reference Code:** New file `src/shared/src/port.rs`

**Acceptance Criteria:**
- [ ] Create `Port` struct wrapping `web_sys::MessagePort`
- [ ] Implement `from_receiver(port: MessagePort) -> mpsc::Receiver<Vec<u8>>` method
- [ ] Use `Closure::wrap` to bridge JS `onmessage` to Rust `mpsc::channel`
- [ ] Channel buffer size of 10 for natural backpressure
- [ ] Add `port.rs` to `src/shared/src/lib.rs` exports
- [ ] Build passes with `npm run build`
- [ ] Typecheck passes

**Implementation Guidance:**
```rust
// src/shared/src/port.rs
use wasm_bindgen::prelude::*;
use web_sys::MessageEvent;
use js_sys::Uint8Array;
use futures::channel::mpsc;

pub struct Port;

impl Port {
    pub fn receiver(port: web_sys::MessagePort) -> mpsc::Receiver<Vec<u8>> {
        let (tx, rx) = mpsc::channel(10);
        let closure = Closure::wrap(Box::new(move |e: MessageEvent| {
            let data = Uint8Array::from(e.data());
            let mut vec = vec![0u8; data.length() as usize];
            data.copy_to(&mut vec);
            let _ = tx.try_send(vec);
        }) as Box<dyn FnMut(_)>);
        port.set_onmessage(Some(closure.as_ref().unchecked_ref()));
        closure.forget();
        rx
    }
    
    pub fn send(port: &web_sys::MessagePort, data: &[u8]) {
        let arr = Uint8Array::from(data);
        let _ = port.post_message_with_transferable(&arr, &arr.buffer());
    }
}
```

---

### US-002: Setup MessageChannels in TypeScript main thread

**Description:** As a developer, I need to create and transfer MessageChannel ports to all workers during initialization.

**Reference Code:** `src/index.ts` lines 62-153 (NostrManager constructor)

**Acceptance Criteria:**
- [ ] Create 6 MessageChannels: `cacheToConn`, `parserToCache`, `connToParser`, `parserToCrypto`, `cryptoToConn`, `cryptoToMain`
- [ ] Transfer correct port ends to each worker's init message
- [ ] Remove SAB creation for: `ws_request`, `ws_response`, `ws_crypto_request`, `ws_crypto_response`, `cache_request`, `cache_response`, `crypto_request`, `crypto_response`, `dbRing`
- [ ] Keep SAB for: `statusRing` (connections→main), per-subscription buffers (parser→main)
- [ ] Update `InitConnectionsMsg`, `InitCacheMsg`, `InitParserMsg`, `InitCryptoMsg` types
- [ ] All workers initialize without errors
- [ ] Build passes

**Channel Topology:**
```
Connections receives: fromCache (port2), fromCrypto (port2), toParser (port1)
Cache receives: fromParser (port2), toConnections (port1), toParser (port1)  
Parser receives: fromConnections (port2), fromCache (port2), fromCrypto (port2), toCache (port1), toCrypto (port1)
Crypto receives: fromParser (port2), toParser (port1), toConnections (port1), toMain (port1)
```

---

### US-003: Migrate Connections worker to MessageChannel

**Description:** As a developer, I need to replace SAB polling with MessageChannel receivers in the Connections worker.

**Reference Code:** 
- `src/connections/index.ts` (worker entry)
- `src/connections/src/lib.rs` lines 37-232 (WSRust struct and polling loop)

**Acceptance Criteria:**
- [ ] Update `WSRust::new()` to accept `MessagePort` parameters instead of SABs
- [ ] Replace `ws_request: Rc<RefCell<SabRing>>` with `from_cache: mpsc::Receiver<Vec<u8>>` and `from_crypto: mpsc::Receiver<Vec<u8>>`
- [ ] Replace `ws_response` SAB writer with `to_parser: web_sys::MessagePort`
- [ ] Replace polling loop (lines 157-230) with `select!` on both receivers
- [ ] Keep `statusRing` SAB writer for main thread status updates
- [ ] Route NIP-46 messages (sub_id starts with "n46:") through `from_crypto` channel
- [ ] Send `WorkerMessage` bytes directly through `to_parser` port
- [ ] Build passes with `npm run build:connections`

**Code Changes:**
```rust
// OLD:
pub struct WSRust {
    ws_request: Rc<RefCell<SabRing>>,
    ws_signer_request: Option<Rc<RefCell<SabRing>>>,
    registry: ConnectionRegistry,
}

// NEW:
pub struct WSRust {
    from_cache: mpsc::Receiver<Vec<u8>>,
    from_crypto: mpsc::Receiver<Vec<u8>>,
    to_parser: web_sys::MessagePort,
    status_ring: Rc<RefCell<SabRing>>, // Keep for now
    registry: ConnectionRegistry,
}

// OLD polling loop (lines 164-230):
// Round-robin polling with exponential backoff

// NEW (select!):
spawn_local(async move {
    loop {
        select! {
            Some(bytes) = from_cache.next() => {
                if let Ok(env) = serde_json::from_slice::<Envelope>(&bytes) {
                    reg.send_to_relays(&env.relays, &env.frames);
                }
            }
            Some(bytes) = from_crypto.next() => {
                if let Ok(env) = serde_json::from_slice::<Envelope>(&bytes) {
                    reg.send_to_relays(&env.relays, &env.frames);
                }
            }
        }
    }
});
```

---

### US-004: Migrate Cache worker to MessageChannel

**Description:** As a developer, I need to replace SAB rings with MessageChannel ports in the Cache worker.

**Reference Code:**
- `src/cache/index.ts` (worker entry)
- `src/cache/src/lib.rs` lines 22-403 (Caching struct)
- `src/cache/src/db/index.rs` lines 25, 128 (ingest_ring usage)

**Acceptance Criteria:**
- [ ] Update `Caching::new()` to accept `MessagePort` parameters
- [ ] Replace `cache_request`, `cache_response`, `ws_request` SABs with ports
- [ ] Replace `db_ring` (ingestRing) - merge into single `from_parser` receiver
- [ ] Update 10 worker loops (lines 372-401) to use `from_parser.next().await`
- [ ] Differentiate message types in `process_local_requests`:
  - If `CacheRequest.event().is_some()` → persist to DB (was db_ring)
  - If `CacheRequest.requests().is_some()` → handle cache lookup (was cache_request)
- [ ] Send cache responses through `to_parser` port as `WorkerMessage` bytes
- [ ] Send REQ frames through `to_connections` port as JSON envelope bytes
- [ ] Build passes with `npm run build:cache`

**Key Change - Message Multiplexing:**
```rust
// SINGLE from_parser receiver handles BOTH:
async fn process_message(&self, bytes: &[u8]) {
    let cache_req = flatbuffers::root::<fb::CacheRequest>(&bytes)?;
    
    if let Some(event) = cache_req.event() {
        // WAS db_ring - persist event
        self.database.persist_event(event).await;
    } else if let Some(requests) = cache_req.requests() {
        // WAS cache_request - handle lookup
        let events = self.database.query(&requests).await;
        self.send_responses(events).await;
    }
}
```

---

### US-005: Migrate Parser worker - CryptoClient to MessageChannel

**Description:** As a developer, I need to update the CryptoClient to use MessageChannel instead of SAB rings.

**Reference Code:** `src/parser/src/crypto_client.rs` lines 1-325

**Acceptance Criteria:**
- [ ] Update `CryptoClient::new()` to accept `MessagePort` instead of SABs
- [ ] Replace `req: Rc<RefCell<SabRing>>` with `to_crypto: web_sys::MessagePort`
- [ ] Replace `resp: Rc<RefCell<SabRing>>` with `from_crypto: mpsc::Receiver<Vec<u8>>`
- [ ] Update `call_raw()` (line 124) to send `SignerRequest` through port
- [ ] Update response pump (lines 60-111) to use `from_crypto.next().await`
- [ ] Keep request/response matching by `request_id` (lines 81-93)
- [ ] Build passes

---

### US-006: Migrate Parser worker - NetworkManager distributor

**Description:** As a developer, I need to replace the SAB distributor task with MessageChannel `select!`.

**Reference Code:** `src/parser/src/network/mod.rs` lines 310-458 (start_response_reader)

**Acceptance Criteria:**
- [ ] Replace `ws_response: Rc<RefCell<SabRing>>` with `from_connections: mpsc::Receiver<Vec<u8>>`
- [ ] Replace `cache_response: Rc<RefCell<SabRing>>` with `from_cache: mpsc::Receiver<Vec<u8>>`
- [ ] Remove `prefer_cache` round-robin logic
- [ ] Remove exponential backoff (`sleep_ms`, `empty_backoff_ms`)
- [ ] Use `select!` to race `from_connections` and `from_cache`
- [ ] Keep existing sharding logic (lines 325-362, 418-446) unchanged
- [ ] Keep `ShardSource` enum to track message origin for debugging
- [ ] Build passes

**Critical Section:**
```rust
// OLD (lines 364-457):
// Polling loop with backoff, round-robin preference

// NEW:
spawn_local(async move {
    loop {
        select! {
            Some(bytes) = from_connections.next() => {
                Self::route_to_shard(subs.clone(), bytes, ShardSource::Network).await;
            }
            Some(bytes) = from_cache.next() => {
                Self::route_to_shard(subs.clone(), bytes, ShardSource::Cache).await;
            }
        }
    }
});
```

---

### US-007: Migrate Parser worker - main lib.rs

**Description:** As a developer, I need to update the Parser worker initialization and message handling.

**Reference Code:** `src/parser/src/lib.rs` lines 92-349

**Acceptance Criteria:**
- [ ] Update `NostrClient::new()` signature to accept all `MessagePort` parameters
- [ ] Create `Port::receiver()` for each inbound port
- [ ] Pass receivers to `NetworkManager::new()`
- [ ] Pass ports to `CryptoClient::new()`
- [ ] Remove `db_ring` parameter (now merged into cache channel)
- [ ] Update `open_subscription` (line 460) to send `CacheRequest` through port
- [ ] Build passes with `npm run build:parser`

---

### US-008: Migrate Crypto worker to MessageChannel

**Description:** As a developer, I need to replace SAB rings with MessageChannel in the Crypto worker.

**Reference Code:**
- `src/crypto/index.ts` (worker entry)
- `src/crypto/src/lib.rs` lines 57-727 (Crypto struct)
- `src/crypto/src/signers/nip46/mod.rs` (NIP-46 SAB usage)

**Acceptance Criteria:**
- [ ] Update `Crypto::new()` to accept `MessagePort` parameters
- [ ] Replace `svc_req/svc_resp` SABs with `from_parser/to_parser` ports
- [ ] Replace `ws_req/ws_resp` SABs with `to_connections/from_connections` ports
- [ ] Update service loop (lines 484-705) to use `select!` on `from_parser`
- [ ] Update NIP-46 transport to use ports instead of SAB
- [ ] Send control responses (get_pubkey, sign_event) through `to_main` port
- [ ] Build passes with `npm run build:crypto`

---

### US-009: Update SaveToDbPipe to use MessageChannel

**Description:** As a developer, I need to update the SaveToDbPipe to send through the MessageChannel.

**Reference Code:** `src/parser/src/pipeline/pipes/save_to_db.rs` lines 1-56

**Acceptance Criteria:**
- [ ] Replace `db_ring: Rc<RefCell<SabRing>>` with `to_cache: web_sys::MessagePort`
- [ ] Update `SaveToDbPipe::new()` to accept port
- [ ] In `process()`, send `CacheRequest` with `event` field set through port
- [ ] Build passes

---

### US-010: Integration testing - Full build and verification

**Description:** As a developer, I need to verify the entire system works end-to-end.

**Acceptance Criteria:**
- [ ] `npm run build` completes without errors
- [ ] All WASM crates compile successfully
- [ ] TypeScript bundle builds without errors
- [ ] Worker initialization logs show correct port transfers
- [ ] No SAB-related errors in console
- [ ] Test subscription flow: main → parser → cache → parser → main
- [ ] Test publish flow: main → parser → crypto → parser → cache → connections
- [ ] Test event ingestion: connections → parser → cache (persist)

**Testing Commands:**
```bash
npm run build
# Check for any SAB-related deprecation warnings
# Run in browser and check console for worker init messages
```

---

## Functional Requirements

- FR-1: All worker-to-worker SAB rings are replaced with MessageChannel ports
- FR-2: No polling loops remain in worker communication paths
- FR-3: `db_ring` and `cache_request` are merged into single `parser→cache` channel
- FR-4: Connections worker uses `select!` to receive from cache and crypto simultaneously
- FR-5: Parser distributor uses `select!` to receive from connections and cache
- FR-6: All existing FlatBuffer message schemas work unchanged
- FR-7: Main thread SAB communication (subscriptions, status) remains functional
- FR-8: Backpressure is handled naturally by MessageChannel queue limits

## Technical Considerations

### Performance
- MessageChannel throughput: 50K-200K msg/sec (vs SAB 1M+ msg/sec)
- For Nostr workloads (10-1000 events/sec), this is more than sufficient
- Latency improves: no polling backoff, immediate wake on message

### Browser Compatibility
- MessageChannel: Universal support (Chrome 4+, Firefox 41+, Safari 5+)
- Transferable objects: Same support
- Web Workers with modules: Modern browsers (we already require this)

### Memory Management
- Ports are automatically closed when workers terminate
- No explicit cleanup needed in normal operation
- `Closure::forget()` in port.rs is intentional (port lives as long as worker)

### Error Handling
- Channel disconnection (worker crash) will cause `next()` to return `None`
- `select!` loop should exit gracefully if all channels close
- Consider adding `warn!` logs for unexpected disconnections

## Migration Checklist

### Before Starting
- [ ] Back up current working code
- [ ] Ensure `npm run build` works on current main
- [ ] Review this PRD with team

### During Implementation
- [ ] Implement stories in order (US-001 through US-010)
- [ ] Build after EACH story to catch errors early
- [ ] Test worker initialization after US-002
- [ ] Test cache lookup after US-004
- [ ] Test crypto operations after US-008

### After Completion
- [ ] Full integration test
- [ ] Performance comparison (optional)
- [ ] Code review
- [ ] Remove SAB-related utilities if no longer needed (separate cleanup PR)

## Open Questions

1. Should we add explicit port closing on worker cleanup/shutdown?
2. Do we need metrics/logging for channel queue depths?
3. Should we add timeout handling for crypto operations?
4. Is the 10-message channel buffer sufficient, or should it be configurable?
