# Research: Lynx Native-to-JS Persistent Event Streaming

## Executive Summary

**Lynx HAS a proper persistent native-to-JS event streaming mechanism.** The current workaround in `LynxNipworkerModule.mm` (re-registering a one-shot callback after every event) is confirmed by official Lynx documentation and a GitHub issue to be the wrong abstraction. The intended pattern for streaming events from native modules to JS is:

- **Native side**: `sendGlobalEvent(eventName, params)` on `LynxContext` or `LynxView`
- **JS side**: `lynx.getJSModule("GlobalEventEmitter").addListener(eventName, listener)`

This eliminates the callback re-registration loop, the native-side event queue, the race window, and the confusing dual-purpose `init` API.

---

## 1. Evidence: Official Lynx Documentation

### Source: lynxjs.org — Event Propagation Guide
URL: https://lynxjs.org/guide/interaction/event-handling/event-propagation

> "Sometimes, developers may need to pass events between different elements and components, or **need to pass events between the client and the front end**, and do not rely on the element to register event listeners. At this time, developers can use `GlobalEventEmitter` to achieve **global scope transmission of events in a page**."

The documentation explicitly provides client-side (native) examples for all three platforms:

**iOS:**
```objc
// You can call the sendGlobalEvent function of LynxContext
[LynxContext sendGlobalEvent:@"eventName" withParams:args];
// Or call the sendGlobalEvent function of LynxView
[LynxView sendGlobalEvent:@"eventName" withParams:args];
```

**Android:**
```java
// You can call the sendGlobalEvent function of LynxContext
LynxContext.sendGlobalEvent("eventName", args);
// Or call the sendGlobalEvent function of LynxView
LynxView.sendGlobalEvent("eventName", args);
```

**HarmonyOS:**
```js
// You can call the sendGlobalEvent function of LynxContext
LynxContext.sendGlobalEvent('eventName', args);
```

### Source: lynxjs.org — sendGlobalEvent API Reference
URL: https://lynxjs.org/api/lynx-native-api/lynx-context/send-global-event

> "Send global events to the front end through the client, and the front end can listen to the event through `GlobalEventEmitter`."

**iOS signature:**
```objc
- (void)sendGlobalEvent:(nonnull NSString *)name withParams:(nullable NSArray *)params;
```

**Android signature:**
```java
public void sendGlobalEvent(String name, JavaOnlyArray params);
```

---

## 2. Evidence: Official Lynx GitHub Issue

### Source: lynx-family/lynx — Issue #1972
URL: https://github.com/lynx-family/lynx/issues/1972
Title: "[Bug]: Android, native module Callback can be invoked only once"

This is an official bug report describing **exactly the same problem** NIPWorker is facing. A developer tries to invoke a `Callback` multiple times from a native module, and only the first invocation works.

**Official Lynx maintainer response:**
> "If you need to invoked the JS callback multiple times, **you can use sendGlobalEvent**."

This is the authoritative answer from the Lynx maintainers: for persistent streaming from native to JS, **do not use callback parameters — use `sendGlobalEvent` instead.**

---

## 3. Evidence: TypeScript Type Definitions (in node_modules)

### File: `node_modules/@lynx-js/types/types/background-thread/event.d.ts`

```typescript
export interface EventEmitter {
  addListener(eventName: string, listener: (...args: unknown[]) => void, context?: object): void;
  removeListener(eventName: string, listener: (...args: unknown[]) => void): void;
  emit(eventName: string, data: unknown): void;
  removeAllListeners(eventName?: string): void;
  trigger(eventName: string, params: string | Record<any, any>): void;
  toggle(eventName: string, ...data: unknown[]): void;
}

export type GlobalEventEmitter = EventEmitter;
```

### File: `node_modules/@lynx-js/types/types/background-thread/lynx.d.ts`

```typescript
export interface Lynx extends CommonLynx {
  getJSModule(name: 'GlobalEventEmitter'): GlobalEventEmitter;
  // ...
}
```

### File: `node_modules/@lynx-js/types/types/background-thread/native-modules.d.ts`

```typescript
export interface NativeModules {
  bridge: {
    call: (name: string, params: Record<string, unknown>, cb: (...args: unknown[]) => void) => void;
    on: (name: string, cb: (...args: unknown[]) => void) => void;
  };
}
```

The `bridge.on` API on `NativeModules` is another persistent listener pattern, but `GlobalEventEmitter` is the documented and recommended approach for cross-layer event streaming.

---

## 4. Evidence: ReactLynx Hook for Global Events

### File: `node_modules/@lynx-js/react/runtime/lib/hooks/useLynxGlobalEventListener.d.ts`

```typescript
export declare function useLynxGlobalEventListener<T extends (...args: any[]) => void>(eventName: string, listener: T): void;
```

Usage example from docs:
```tsx
import { useLynxGlobalEventListener } from "@lynx-js/react";

useLynxGlobalEventListener("onWindowResize", (e) => {
  console.log("Window resized:", e);
});
```

This hook is a thin wrapper around `lynx.getJSModule('GlobalEventEmitter').addListener(...)` with automatic cleanup.

---

## 5. Current Workaround vs. Intended Pattern

### Current (Workaround)

```
JS calls init(callback)
native stores callbackBlock
Rust emits one event
native invokes callbackBlock → Lynx ERASES it from internal map
JS callback handles event
JS calls init(callback) again
native flushes one queued event if any
repeat
```

Problems:
- Extra bridge registration for every native event
- Race window between callback invocation and JS re-registration
- Requires native-side buffering (`pendingEvents`) to avoid dropped events
- Unbounded `pendingEvents` memory growth
- Confusing API: `init` means both "start engine" and "register next callback"
- Diagnostic methods (`testPing`, `getCallbackStatus`) are temporary debugging crutches
- Already-dispatched callbacks can still run after `deinit`

### Intended (Lynx Pattern)

```
JS calls lynx.getJSModule('GlobalEventEmitter').addListener('NipworkerEvent', handler)
native calls [LynxContext sendGlobalEvent:@"NipworkerEvent" withParams:@[data]]
JS handler receives event
repeat — no re-registration, no buffering, no race window
```

Benefits:
- **Persistent listener**: Register once, receive forever
- **No native-side queue**: Events are delivered directly through Lynx's internal event pipeline
- **No race conditions**: The listener stays registered until explicitly removed
- **Clean API separation**: `init` starts the engine; events flow through a dedicated channel
- **Removes temporary diagnostics**: `testPing` and `getCallbackStatus` become unnecessary
- **Scales to all platforms**: Same pattern works on iOS, Android, and HarmonyOS

---

## 6. Migration Path

### JS Side (`src/NativeBackend.ts`)

Replace the callback re-registration loop:

```typescript
// BEFORE
const registerCallback = () => {
  this.nativeModule.init((data: ArrayBuffer) => {
    this.handleNativeMessage(new Uint8Array(data));
    registerCallback(); // ← re-register for next event
  });
};
registerCallback();
```

With a single `GlobalEventEmitter` listener:

```typescript
// AFTER
const eventEmitter = globalThis.lynx?.getJSModule?.('GlobalEventEmitter');
if (!eventEmitter) {
  throw new Error('[NativeBackend] GlobalEventEmitter not available');
}

const listener = (data: ArrayBuffer) => {
  this.handleNativeMessage(new Uint8Array(data));
};
eventEmitter.addListener('NipworkerEvent', listener);

// Store for cleanup
this._eventListener = listener;
this._eventEmitter = eventEmitter;

// init() now ONLY starts the engine
this.nativeModule.init();
```

Cleanup on deinit:
```typescript
if (this._eventEmitter && this._eventListener) {
  this._eventEmitter.removeListener('NipworkerEvent', this._eventListener);
}
```

### iOS Side (`crates/native-ffi/ios/LynxNipworkerModule.mm`)

Replace the callback-based event forwarding with `sendGlobalEvent`.

The key question is: **how does the native module access `LynxContext` or `LynxView`?**

**Option A: Via LynxContext (preferred if available)**

If the `LynxModule` protocol provides access to the `LynxContext` (e.g., through an `initWithParam:` or a `lynxContext` property), the module can call:

```objc
// In the callback forwarder, instead of invoking callbackBlock:
[LynxContext sendGlobalEvent:@"NipworkerEvent" withParams:@[data]];
```

**Option B: Via host app LynxView**

The host app that creates the `LynxView` can hold a reference to it and expose it to the module, or the module can delegate event sending to the host app.

**Option C: LynxModule receives context in constructor (Android pattern)**

On Android, `LynxModule(context: Context)` receives the context. The module can cast it to `LynxContext` and call `sendGlobalEvent`.

```kotlin
class NipworkerLynxModule(context: Context) : LynxModule(context) {
    // ...
    fun onNativeData(userdata: Long, data: ByteArray) {
        (context as? LynxContext)?.sendGlobalEvent("NipworkerEvent", JavaOnlyArray().apply {
            // push data
        })
    }
}
```

For iOS, the `LynxModule` protocol likely has a similar mechanism. The `NativeLocalStorageModule` example in the Lynx docs uses `public init(param: Any)`, suggesting the runtime passes context information during initialization.

### Android Side (`crates/native-ffi/android/LynxNipworkerModule.kt`)

Currently the Android module stores the callback in a `ConcurrentHashMap` and invokes it. This will have the same one-shot limitation. It should be migrated to `LynxContext.sendGlobalEvent()` as shown in Option C above.

---

## 7. Important Caveats

### Data type limitations
`sendGlobalEvent` params must be JSON-serializable types (arrays, dictionaries, strings, numbers, booleans). Raw `NSData` / `ByteArray` cannot be sent directly.

**Workaround**: Base64-encode the binary payload:
```objc
NSString *b64 = [data base64EncodedStringWithOptions:0];
[LynxContext sendGlobalEvent:@"NipworkerEvent" withParams:@[b64]];
```

JS side:
```typescript
const listener = (b64: string) => {
  const binary = Uint8Array.from(atob(b64), c => c.charCodeAt(0));
  this.handleNativeMessage(binary);
};
```

This adds a Base64 encode/decode step, which is a small overhead compared to the current callback re-registration + queueing overhead.

### Thread safety
The iOS `sendGlobalEvent` documentation examples dispatch to the main queue for UI events, but `GlobalEventEmitter` events are handled on the background thread (BTS). The native module should verify whether `sendGlobalEvent` can be called from background threads or if it needs main-queue dispatch. Given that the Rust engine runs on a background thread, the module should ensure thread safety.

---

## 8. References

| Source | URL | Relevance |
|--------|-----|-----------|
| Lynx Event Propagation Guide | https://lynxjs.org/guide/interaction/event-handling/event-propagation | Documents `GlobalEventEmitter` and `sendGlobalEvent` |
| Lynx sendGlobalEvent API (Context) | https://lynxjs.org/api/lynx-native-api/lynx-context/send-global-event | API signatures for all platforms |
| Lynx sendGlobalEvent API (View) | https://lynxjs.org/3.5/api/lynx-native-api/lynx-view/send-global-event.html | iOS/Android/Harmony signatures |
| Lynx GitHub Issue #1972 | https://github.com/lynx-family/lynx/issues/1972 | Official confirmation: "use sendGlobalEvent" for multiple callbacks |
| `@lynx-js/types` event.d.ts | `node_modules/@lynx-js/types/types/background-thread/event.d.ts` | `EventEmitter` / `GlobalEventEmitter` interface |
| `@lynx-js/types` lynx.d.ts | `node_modules/@lynx-js/types/types/background-thread/lynx.d.ts` | `getJSModule('GlobalEventEmitter')` |
| `@lynx-js/react` hook | `node_modules/@lynx-js/react/runtime/lib/hooks/useLynxGlobalEventListener.d.ts` | React hook for global event listening |

---

## 9. Conclusion

**Lynx explicitly supports persistent native-to-JS event streaming via `sendGlobalEvent` + `GlobalEventEmitter`.** The current callback re-registration workaround is:
1. **Confirmed by official docs** to be the wrong pattern for streaming
2. **Confirmed by an official GitHub issue** (#1972) as the exact anti-pattern to avoid
3. **Replaceable** with a cleaner architecture that removes buffering, race windows, and temporary diagnostics

The migration requires:
1. **JS side**: Replace `nativeModule.init(callback)` loop with `GlobalEventEmitter.addListener('NipworkerEvent', handler)`
2. **Native side**: Replace callback invocation with `sendGlobalEvent` (may require Base64 encoding for binary data)
3. **Remove**: `pendingEvents`, `callbackBlock`, `flushQueuedEvents`, `testPing`, `getCallbackStatus`
