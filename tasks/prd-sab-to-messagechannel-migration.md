# PRD: Migrate from SharedArrayBuffer to MessageChannel

## Introduction

NIPWorker currently uses SharedArrayBuffer (SAB) for high-performance communication between the main thread and the parser worker. However, SAB requires strict browser security policies (HTTPS, COOP/COEP headers) that limit deployment flexibility. This PRD outlines the migration to MessageChannel with transferable ArrayBuffers, maintaining near-SAB performance while removing browser restrictions.

## Goals

- Remove SharedArrayBuffer dependency entirely
- Maintain zero-copy event transfer using transferable ArrayBuffers
- Implement efficient batching to minimize MessageChannel overhead
- Preserve the existing hooks API (`useSubscription`, `usePublish`)
- Ensure performance remains comparable to SAB implementation

## User Stories

### US-001: Replace SAB with MessageChannel in NostrManager
**Description:** As a developer, I want NostrManager to use MessageChannel instead of SharedArrayBuffer so that the library works without COOP/COEP headers.

**Acceptance Criteria:**
- [ ] NostrManager creates a `MessageChannel` for each subscription/publish instead of SAB
- [ ] The `MessagePort` is transferred to the parser worker during subscription
- [ ] Local `ArrayBuffer` is created to store incoming events (not SharedArrayBuffer)
- [ ] Typecheck passes

### US-002: Implement Batched Event Delivery
**Description:** As a developer, I want events to be batched before sending through MessageChannel to minimize overhead and maintain performance.

**Acceptance Criteria:**
- [ ] Parser worker accumulates events in a buffer (e.g., 16KB or 50ms timeout)
- [ ] Batched events are sent as a single Uint8Array with framing
- [ ] Framing format: `[4-byte count][4-byte len][event data][4-byte len][event data]...`
- [ ] Typecheck passes

### US-003: Update Parser Worker to Send via MessagePort
**Description:** As a developer, I want the parser worker to send events through MessagePort instead of writing to SAB.

**Acceptance Criteria:**
- [ ] Parser `NostrClient` accepts a `MessagePort` for each subscription
- [ ] `NetworkManager` stores the port per subscription
- [ ] `SharedBufferManager` is replaced/updated to use MessagePort
- [ ] Events are written to a local buffer, batched, and sent via `postMessage` with transferable
- [ ] Typecheck passes

### US-004: Update Hooks to Read from ArrayBuffer
**Description:** As a developer, I want `useSubscription` and `usePublish` to read from a local ArrayBuffer instead of SharedArrayBuffer.

**Acceptance Criteria:**
- [ ] Create `ArrayBufferReader` class (similar to `SharedBufferReader`)
- [ ] `useSubscription` uses `ArrayBufferReader` to read events
- [ ] `usePublish` uses `ArrayBufferReader` to read status
- [ ] Hook callbacks receive `WorkerMessage` objects (same as before)
- [ ] Typecheck passes

### US-005: Implement Main Thread Event Buffering
**Description:** As a developer, I want the main thread to receive batched events and write them to a local ArrayBuffer for the hooks to consume.

**Acceptance Criteria:**
- [ ] NostrManager receives batched events via `port.onmessage`
- [ ] Events are appended to the subscription's local ArrayBuffer
- [ ] Signal is dispatched to wake up the hook (`dispatch(`subscription:${subId}`)`)
- [ ] Typecheck passes

### US-006: Update FlatBuffer Schemas if Needed
**Description:** As a developer, I want to ensure the FlatBuffer schemas support the new transport if any changes are needed.

**Acceptance Criteria:**
- [ ] Review existing schemas for SAB-specific assumptions
- [ ] Add `BatchedEvents` message type if needed for framing
- [ ] Regenerate TypeScript/Rust bindings with `npm run flatc`
- [ ] Typecheck passes

### US-007: Remove SAB-Related Code
**Description:** As a developer, I want to clean up all SAB-related code after migration.

**Acceptance Criteria:**
- [ ] Remove `SharedBufferReader` class (or deprecate)
- [ ] Remove `SharedBufferManager` from Rust
- [ ] Remove `sab_ring.rs` if no longer needed
- [ ] Update `initializeRingHeader` and `statusRing` usage
- [ ] Typecheck passes

### US-008: Performance Testing
**Description:** As a developer, I want to verify the MessageChannel implementation performs comparably to SAB.

**Acceptance Criteria:**
- [ ] Benchmark event throughput (should be within 20% of SAB)
- [ ] Benchmark latency (should be within 20% of SAB)
- [ ] Test with 1000+ events to ensure no dropped messages
- [ ] Document any performance differences

## Functional Requirements

- FR-1: Replace all `SharedArrayBuffer` usage with `ArrayBuffer` + `MessageChannel`
- FR-2: Implement batching: accumulate events up to a size limit (16KB) or time limit (50ms)
- FR-3: Use transferable objects for zero-copy transfer: `port.postMessage({subId, data}, [data.buffer])`
- FR-4: Maintain same `WorkerMessage` format inside the batch
- FR-5: Keep existing hook APIs unchanged (`useSubscription`, `usePublish`)
- FR-6: Remove all SAB-related code paths
- FR-7: Update build configuration if COOP/COEP headers are no longer needed
- FR-8: Message format from parser: `{ subId: string, data: Uint8Array }` where data contains `[len][msg][len][msg]...`

## Non-Goals

- No changes to worker-to-worker communication (parser↔cache, parser↔connections)
- No changes to crypto worker MessageChannel usage
- No changes to event parsing logic
- No changes to pipeline processing
- No support for both SAB and MessageChannel (complete migration)

## Design Considerations

### Batching Strategy

```
Parser Worker:
1. Accumulate events in local Vec<u8>
2. When buffer > 16KB OR timeout > 50ms:
   a. Send batch via port.postMessage
   b. Clear local buffer

Main Thread:
1. Receive batch via port.onmessage
2. Append to subscription's ArrayBuffer
3. Dispatch signal to hooks
4. Hook reads from ArrayBuffer via ArrayBufferReader
```

### Message Format

Each batch contains multiple FlatBuffer messages with length prefix:
```
[4-byte: number of events]
[4-byte: event 1 length][event 1 data]
[4-byte: event 2 length][event 2 data]
...
```

### ArrayBuffer Layout (same as current SAB layout)

```
[0-3]: Write position (4 bytes, little endian)
[4+]: [4-byte length][FlatBuffer message][4-byte length][FlatBuffer message]...
```

This allows reusing most of the `SharedBufferReader` logic.

## Technical Considerations

- **Zero-copy**: Use `Uint8Array` view over `ArrayBuffer` and transfer the underlying buffer
- **Backpressure**: If ArrayBuffer fills up, apply same `BufferFull` logic as current SAB
- **Memory management**: ArrayBuffer can grow dynamically or use fixed size with ring semantics
- **Threading**: Main thread is single-threaded, no race conditions on ArrayBuffer writes

## Open Questions

1. Should we use a fixed-size ArrayBuffer with ring semantics or a growable one?
2. What are the optimal batch size and timeout values (16KB/50ms are estimates)?
3. Do we need flow control (parser waits if main thread is slow)?
