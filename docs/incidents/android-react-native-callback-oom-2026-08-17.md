# Android React Native callback OOM — 2026-08-17

Status: resolved upstream for the 0.99.11 release.

## Summary

Crays hit repeated `java.lang.OutOfMemoryError: bad_array_new_length` failures while
validating `@candypoets/nipworker` 0.99.6 on Android. The failures originate in the
React Native codegen callback conversion path:

```text
CxxCallbackImpl.nativeInvoke
NativeNipworkerReactNativeSpec.emitOnData
NipworkerReactNativeModule.emitData
NipworkerReactNativeModule.emitRuntimeData
NipworkerRuntime.dispatch
NipworkerReactNativeModule.onNativeData
```

This is not attributed to ordinary heap pressure. Seven native callback threads fail
(`Thread-7`, `Thread-9` through `Thread-14`), including pairs failing in the same
millisecond. The retained run reached the failure before the room join screen. Android
assembly and relay teardown succeeded. The separate Test Room empty-state issue was
caused by Crays missing nipworker's asynchronous restored-session auth callback: the
manager was constructed behind `RuntimeGate`, but the callback was not retained for the
later `RoomDataProvider` mount. `ReactNativeBackend` already restored its persisted
session and emitted auth. That Crays lifecycle issue is out of scope here.

## Environment

- Crays commit `0ff95759a2311cf021a2f90c76461edd63218cd8`, with its uncommitted
  Test Room fix present
- Expo `~57.0.8`
- React Native `0.86.2`, new architecture enabled
- Hermes enabled
- Android 34 Google APIs x86_64 debug development client
- `@candypoets/nipworker` 0.99.6
- Node 22.12.0 and OpenJDK 17.0.18

## Retained evidence

The source incident report is:

`/root/code/crays-rn/docs/workflows/nipworker-android-oom-2026-08-17.md`

Raw evidence is retained under:

`/root/.maestro/tests/2026-08-17_131023/01-people/`

| File | Bytes | SHA-256 |
| --- | ---: | --- |
| `logs/crash-report.txt` | 1,893 | `c06eb8a04bea668f77bedf33f54c53d8699f4077ba83208824e234e2804a48b6` |
| `logs/device-logcat.txt` | 309,073 | `309da1f20e1867388f6d841537216ea2bae56469ea90677537bacb6e1cd1aa5c` |
| `screenshots/step-013-assertCondition-join-privacy-screen.png` | 127,843 | `daac82cec0e3d7a0539806220d27a5613c1ec74fb3477a76839551b09a8f5c4c` |
| `commands.json` | 17,115 | `768d1f1688569a86085c54d902a9892e930ba4038b1cb6e6f343d7d4e18eb43d` |

Do not modify these retained files. Do not copy NIP-46 secrets from adjacent logs into
this record.

## 0.99.6 versus 0.99.8

The Android callback boundary is byte-for-byte identical in tags `v0.99.6` and
`v0.99.8`. The relevant Git blobs are:

| Path | Blob in both tags |
| --- | --- |
| `src/specs/NativeNipworkerReactNative.ts` | `b965d48316035e4c1acc2f461e5cc0f5b17d15f2` |
| `crates/native-ffi/react-native/android/src/main/java/com/candypoets/nipworker/reactnative/NipworkerReactNativeModule.kt` | `53b440a67431c7c3e2b626595dc1c5790760c6dc` |
| `crates/native-ffi/android/nipworker_jni_impl.c` | `5b8605f6a8fe7f444bc6dbd3b331041b28fcd06f` |
| `src/react-native.ts` | `33b877469b6cabec679b03c834076264a6f6dbb3` |

Therefore 0.99.8's module-packaging changes do not fix this callback crash. A test with
0.99.8 remains useful as a reproduction confirmation, but no relevant source delta exists
that could make a different result expected.

This incident is also distinct from the earlier 0.99.3 queued-wake schema mismatch.
Commit `8564604` (included since 0.99.5) made `data` optional in the TurboModule event
schema and emits `data: []` for `{ v: 1, encoding: "queued" }` compatibility. Both
0.99.6 and 0.99.8 already contain that fix.

## Initial upstream analysis

The current JNI callback copies Rust-owned bytes into a JVM `byte[]` before freeing the
Rust allocation, so the obvious byte-buffer lifetime is bounded correctly. It does cast
`size_t len` to `jsize` without an explicit upper-bound check; that should be hardened,
but the small startup messages and the stack location do not make an oversized Rust
buffer the leading explanation.

The leading hypothesis is unsafe callback scheduling:

1. Rust callbacks attach their current native thread and call Kotlin `onNativeData`
   synchronously.
2. `NipworkerRuntime.dispatch` snapshots listeners under a lock, but invokes them on the
   caller's native thread.
3. The module listener calls `emitRuntimeData`, which directly calls the generated
   `emitOnData` callback.
4. The evidence shows concurrent callbacks from multiple native threads entering the
   React Native event-emitter conversion path.

React Native codegen callbacks should be serialized onto the appropriate React/JS queue
before invoking `emitOnData`. This hypothesis still requires an Android reproduction or
a focused regression test before it is promoted to confirmed root cause.

## Required upstream work

- Serialize or marshal native runtime callbacks onto the React/JS queue before invoking
  the generated event emitter.
- Preserve queued-byte-runtime semantics: a queued wake must contain a valid compatibility
  array while the actual bytes remain in the JSI queue.
- Reject callback lengths that cannot fit in `jsize` before `NewByteArray` and clear or
  report JNI exceptions deterministically.
- Add a startup/burst regression covering simultaneous native callbacks under the new
  architecture and Hermes-compatible payload conversion.
- Re-run the Crays Android scenario with the fixed package; no Crays-side change is
  requested before an upstream candidate exists.

## Resolution

The callback/emitter architecture was removed rather than patched. Android and iOS now
compile the same C++ runtime-scoped transport. Rust-owned subscription payloads remain in
fixed-capacity buffers exposed through opaque, ref-counted JSI `ArrayBuffer` pins. Direct
control packets transfer their Rust allocation into the JSI `ArrayBuffer` without an
intermediate JNI, Kotlin, Objective-C, or JavaScript payload copy.

Native producers mark bounded dirty route IDs or enqueue bounded control packets. A single
atomic outer wake schedules `CallInvoker::invokeAsync`; JavaScript drains all pending work.
The clear/recheck protocol covers both arrivals racing wake completion and handler
installation racing an initially absent scheduler. The generated `onData` emitter,
`NativeEventEmitter` fallback, JNI `jbyteArray` callback, process-global listener/queue,
iOS notification/main-queue wake, and Swift borrowed React Native handle were deleted.

Runtime invalidation clears handlers and queues, releases logical subscription leases, and
unbinds its generation before engine teardown. Native deinit now signals and joins the engine
thread plus parser, connections, cache, and crypto workers. A real FFI lifecycle probe that
previously retained exactly five threads per cycle now has zero thread growth after both 25
and 100 destroy/recreate cycles; retained callback count cannot change after deinit returns.

Verification for the replacement includes the shared 10,000-event multithread burst,
duplicate-route coalescing, runtime recreation with late arrivals, both wake races, bounded
route/control saturation, ownership release, sanitizer runs, exact Android React Native 0.86.2
compilation, and source parity guards for Android, iOS, Swift, and TypeScript. Measured results
are recorded in `docs/benchmarks/react-native-native-delivery-2026-08-21.md`.

## Delegation

- `native-mobile-android-callback-oom`: native boundary audit, fix, and Android validation
- `perf-lab-android-callback-oom`: burst/concurrency regression and independent version
  comparison
