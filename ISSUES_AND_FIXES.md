# Nipworker Issues & Fixes — Lynx/Native iOS Migration

This document catalogs all changes made to `nipworker` while integrating it into a Sparkling/Lynx iOS app. Use this as a checklist for upstreaming fixes.

---

## 🔴 CRITICAL BUGS — Must fix in upstream

### 1. `search` field unconditionally encoded as FlatBuffers byte array

**Files:**
- `src/NativeBackend.ts`
- `src/NostrManager.ts`
- `src/EngineManager.ts`

**Problem:**
When `r.search` is `undefined`, `this.textEncoder.encode(undefined)` produces a valid `Uint8Array` containing the bytes for the string `"undefined"`. FlatBuffers serializes this as a non-empty search string. Relays that don't support the `search` NIP extension reject the subscription with:

```
unrecognised filter item: search
```

**Fix (same pattern in all 3 files):**

```typescript
// BEFORE
this.textEncoder.encode(r.search),

// AFTER
r.search ? this.textEncoder.encode(r.search) : null,
```

---

### 2. Blocking `std::net::TcpStream::connect` in async context on `current_thread` runtime

**File:** `crates/native-ffi/src/transport.rs`

**Problem:**
A fallback connection path used `std::net::TcpStream::connect` (blocking) directly inside the async context. On a `current_thread` tokio runtime with `LocalSet`, this freezes the entire OS thread for ~75 seconds when connecting to unreachable IPs, preventing **all** other async I/O (including `tokio::net::TcpStream::connect` timers) from making progress.

This manifested as "all relays timeout" when in reality only one relay was unreachable — the unreachable connection hogged the thread.

**Fix:**
Remove any `spawn_blocking` + `std::net::TcpStream::connect` diagnostic path. Keep only the async `tokio::net::TcpStream::connect` wrapped in `tokio::time::timeout`. The `open_websocket()` function introduced during debugging already does this correctly.

**Key principle:** On `current_thread` + `LocalSet`, never use blocking std APIs in async contexts. Always use `tokio::time::timeout(tokio::net::TcpStream::connect(addr))`.

---

### 3. `nostr-tools` `nip54.js` uses Unicode property escapes incompatible with QuickJS/PrimJS

**File:** `node_modules/nostr-tools/lib/esm/nip54.js` (or wherever bundled into dist)

**Problem:**
`normalizeIdentifier` uses regexes with Unicode property escapes:

```javascript
/\p{Letter}/u.test(char) || /\p{Number}/u.test(char)
```

QuickJS (and PrimJS, the JS engine used by Lynx) does **not** support `\p{...}` Unicode property escapes. This causes a **compile-time error** in the Lynx bundle build:

```
SyntaxError: invalid escape sequence in regular expression
    at main-thread.js:1682:3
```

**Fix:**
Replace with an ASCII-compatible character class. For nostr identifiers/handles, ASCII letters and digits are sufficient:

```javascript
// BEFORE (nip54.js line 6)
if (/\p{Letter}/u.test(char) || /\p{Number}/u.test(char)) {

// AFTER
if (/[A-Za-z0-9]/.test(char)) {
```

> **Note:** If you bundle `nostr-tools` via esbuild/vite for Lynx consumers, this regex will break the Lynx build. Either patch the dependency or vendor a replacement `normalizeIdentifier` function.

---

## 🟡 iOS / Native Platform Compatibility

### 4. `tokio` needs the `"net"` feature for `tokio::net::TcpStream`

**File:** `crates/native-ffi/Cargo.toml`

```toml
# BEFORE
tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros"] }

# AFTER
tokio = { version = "1", features = ["rt-multi-thread", "sync", "time", "macros", "net"] }
```

---

### 5. `tokio-tungstenite` TLS backend should use `native-tls` on iOS

**File:** `crates/native-ffi/Cargo.toml`

```toml
# BEFORE
tokio-tungstenite = { version = "0.24", features = ["rustls-tls-webpki-roots"] }

# AFTER
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
```

**Why:** `native-tls` uses Apple's Security.framework on iOS, which works out of the box. `rustls-tls-webpki-roots` requires `ring` and bundled webpki root certs, which can be problematic in the iOS sandbox and add binary bloat.

---

### 6. iOS logging via `tracing-oslog`

**File:** `crates/native-ffi/Cargo.toml`

```toml
[dependencies]
tracing-oslog = "0.3"
```

**File:** `crates/native-ffi/src/lib.rs` — initialize in `nipworker_init()`:

```rust
#[cfg(target_vendor = "apple")]
{
    use tracing_subscriber::prelude::*;
    let _ = tracing_subscriber::registry()
        .with(tracing_oslog::OsLogger::new("com.nutscash.sparkling", "nipworker"))
        .try_init();
}
#[cfg(not(target_vendor = "apple"))]
{
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .try_init();
}
```

This routes Rust `tracing` logs to the iOS system log, visible in Console.app or via:

```bash
xcrun simctl spawn booted log stream --predicate 'process == "SparklingGo"' --level debug
```

---

### 7. Panic hook so Rust panics aren't silent

**File:** `crates/native-ffi/src/lib.rs`

Add near the top of `nipworker_init()`:

```rust
std::panic::set_hook(Box::new(|info| {
    let backtrace = std::backtrace::Backtrace::capture();
    eprintln!("[nipworker] PANIC: {}", info);
    eprintln!("[nipworker] Backtrace:\n{}", backtrace);
}));
```

Without this, a Rust panic on the native thread kills the engine silently. The JS side just stops receiving events with no explanation, making debugging nearly impossible.

---

## 🟠 LynxJS-Specific Callback Architecture

### 8. Lynx native module callbacks are ONE-SHOT — **RESOLVED via GlobalEventEmitter**

**Files:**
- `crates/native-ffi/ios/LynxNipworkerModule.mm`
- `crates/native-ffi/android/LynxNipworkerModule.kt`
- `src/NativeBackend.ts`

**Problem (historical):**
After the first invocation, Lynx erases the callback from its internal map. All subsequent events from Rust are lost unless the callback is re-registered.

**Resolution:**
Migrated to Lynx's official `sendGlobalEvent` + `GlobalEventEmitter` pattern:

- **Native side** (iOS & Android): base64-encodes the Rust payload and calls `sendGlobalEvent("NipworkerEvent", params)`.
- **JS side**: registers a persistent listener via `lynx.getJSModule("GlobalEventEmitter").addListener("NipworkerEvent", handler)`.

This eliminates the one-shot callback workaround, the native-side event queue (`pendingEvents`), the race window between callback invocation and re-registration, and the diagnostic crutches (`testPing`, `getCallbackStatus`).

See `RESEARCH_LYNX_EVENT_STREAMING.md` for design rationale.



---

### 9. `NativeModules` is a bundle parameter, not on `globalThis`

**Files:** `src/NativeBackend.ts`, `src/manager.ts`

**Problem:**
In Sparkling/Lynx production builds, `NativeModules` is injected as an IIFE argument, not mounted on `globalThis`. The existing `globalThis.lynx?.getNativeModules?.()` lookup fails and the module is not found.

**Fix:**
Check the injected `NativeModules` constant first, then fall back to `globalThis` lookups:

```typescript
/** Lynx injects NativeModules as a bundle parameter, not on globalThis. */
declare const NativeModules: Record<string, any> | undefined;

function getNipworkerModule(): any {
    let mod =
        (typeof NativeModules !== 'undefined' && NativeModules?.NipworkerLynxModule) ||
        globalThis.lynx?.getNativeModules?.()?.NipworkerLynxModule ||
        globalThis.NativeModules?.NipworkerLynxModule;

    if (!mod) {
        try {
            const app = globalThis.lynx?.getNativeApp?.();
            if (app && app.NativeModules) {
                mod = app.NativeModules.NipworkerLynxModule;
            }
        } catch {
            // ignore
        }
    }

    if (!mod) {
        throw new Error(
            '[NativeBackend] NipworkerLynxModule not found. Ensure the native module is registered.'
        );
    }
    return mod;
}
```

Apply the same pattern to `hasLynxNativeModule()` in `manager.ts`.

---

### 10. `queueMicrotask` is not available in QuickJS

**Files:** `src/hooks.ts`, `src/NativeBackend.ts`, `src/NostrManager.ts`, `src/EngineManager.ts`

**Problem:**
QuickJS/PrimJS does not implement `queueMicrotask`. Calling it throws:

```
ReferenceError: queueMicrotask is not defined
```

**Fix:**
Create a `scheduleMicrotask` helper and replace all `queueMicrotask(...)` calls:

```typescript
// src/lib/scheduleMicrotask.ts
export function scheduleMicrotask(callback: () => void): void {
    if (typeof queueMicrotask === 'function') {
        queueMicrotask(callback);
    } else {
        Promise.resolve().then(callback);
    }
}
```

Usage:
```typescript
import { scheduleMicrotask } from './lib/scheduleMicrotask';

// BEFORE
queueMicrotask(() => this.restoreSession());

// AFTER
scheduleMicrotask(() => this.restoreSession());
```

---

### 11. Auto-initialize manager so hooks work without explicit `setManager()`

**File:** `src/manager.ts`

**Problem:**
Hooks call `getManager()` before the app has called `setManager(...)`, causing a hard error.

**Fix:**
Lazy auto-initialization in `getManager()`:

```typescript
let globalManager: NostrManagerLike | null = null;

export function getManager(): NostrManagerLike {
    if (!globalManager) {
        try {
            const { createNostrManager } = require('./native');
            globalManager = createNostrManager();
        } catch {
            throw new Error(
                '[nipworker] Global manager is not set. Call setManager(createNostrManager(...)) before using hooks.'
            );
        }
    }
    return globalManager;
}
```

Also add `setManager(this)` at the end of each backend constructor (`NativeBackend`, `NostrManager`, `EngineManager`) so whichever backend is instantiated first becomes the global manager.

---

## 🟢 TRANSPORT / CONNECTION IMPROVEMENTS

### 12. `open_websocket()` with per-step timeouts and DNS fallback

**File:** `crates/native-ffi/src/transport.rs`

The original `connect_async(url)` was a single shot with no granular timeout. The new `open_websocket()` function provides resilient mobile-network connection handling:

1. **STEP 1:** Try `connect_async` with a 10s timeout (happy path)
2. **STEP 2:** Parse host/port from URL
3. **STEP 3:** Manual DNS lookup with 5s timeout
4. **STEP 4:** Sort addresses IPv4-first
5. **STEP 5:** Build WebSocket request (preserves hostname for TLS SNI)
6. **STEP 6:** Per-address TCP connect (8s timeout) → TLS+WS handshake (8s timeout)

This is more resilient on mobile networks where the high-level `connect_async` can hang indefinitely. Each step has explicit `tracing::info!` / `warn!` / `error!` logs for debugging relay connectivity.

**Key constants:**
```rust
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);  // Step 1
const TCP_TIMEOUT: Duration = Duration::from_secs(8);       // Step 6 TCP
const TLS_WS_TIMEOUT: Duration = Duration::from_secs(8);    // Step 6 WS
```

---

## Summary Checklist

| # | Issue | File(s) | Severity |
|---|-------|---------|----------|
| 1 | `search` encoded even when `undefined` | `NativeBackend.ts`, `NostrManager.ts`, `EngineManager.ts` | 🔴 Critical |
| 2 | Blocking `std::net::TcpStream::connect` in async context | `crates/native-ffi/src/transport.rs` | 🔴 Critical |
| 3 | `\p{Letter}` / `\p{Number}` not supported in QuickJS | `nostr-tools` dep or bundled output | 🔴 Critical |
| 4 | `tokio` missing `"net"` feature | `crates/native-ffi/Cargo.toml` | 🟡 Required for native |
| 5 | `rustls-tls-webpki-roots` → `native-tls` on iOS | `crates/native-ffi/Cargo.toml` | 🟡 Required for native |
| 6 | No iOS log routing for `tracing` | `Cargo.toml`, `src/lib.rs` | 🟡 Required for native |
| 7 | Silent Rust panics | `crates/native-ffi/src/lib.rs` | 🟡 Required for native |
| 8 | Lynx callbacks are one-shot — **resolved** via `GlobalEventEmitter` | `LynxNipworkerModule.mm`, `LynxNipworkerModule.kt`, `NativeBackend.ts` | ✅ Fixed |
| 9 | `NativeModules` not found on `globalThis` | `NativeBackend.ts`, `manager.ts` | 🟠 Lynx-specific |
| 10 | `queueMicrotask` missing in QuickJS | `hooks.ts`, `NativeBackend.ts`, `NostrManager.ts`, `EngineManager.ts` | 🟠 Lynx-specific |
| 11 | Hooks fail without explicit `setManager()` | `manager.ts`, backend constructors | 🟠 Lynx-specific |
| 12 | No granular timeout on WS connect | `crates/native-ffi/src/transport.rs` | 🟢 Improvement |

---

## Notes for Upstreaming

- Items **1–3** are genuine bugs that affect correctness regardless of platform.
- Items **4–7** are needed for any iOS native consumer.
- Items **8–11** are LynxJS-specific but harmless for web/WASM consumers.
- Item **12** is a general robustness improvement worth keeping for all native targets.

Consider creating a `native-ffi` feature flag or a separate `@candypoets/nipworker/native` entry point so web users don't pay the binary cost of native-only code paths.
