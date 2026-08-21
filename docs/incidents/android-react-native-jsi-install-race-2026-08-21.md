# Android React Native JSI install race — 2026-08-21

## Impact

Crays Board's relay-backed journeys crashed during common startup on React
Native 0.86.2/Hermes. One tombstone failed while
`RuntimeTransport::install` mutated the runtime from `mqt_v_native`; another
failed on `mqt_v_js` while creating a host function. The latter occurred before
the Rust engine initialization log, so duplicate engines were not the cause of
this JSI fault.

## Root cause

The TypeScript entry called asynchronous `initEngine` before synchronous
`installByteRuntime`. Android's `initEngine` called `ensureTransport` on the
native-module queue while `installByteRuntime` could call it on the JS queue.
The unsynchronized token allowed both threads to create transports and mutate
the same Hermes runtime concurrently.

## Correction

- The blocking synchronous install now completes before engine startup.
- `initEngine` can only verify an existing transport; it never touches JSI.
- Android serializes token installation and invalidation.
- The shared C++ install gate makes repeated installation idempotent and makes
  teardown wait for an in-flight install.
- iOS serializes its binding install/invalidation with the same checked C++
  installation contract.
- Swift and React Native now acquire one Rust process registry handle instead
  of relying on independent wrapper singletons.

## Regression contract

Host tests cover concurrent repeated install, install/invalidate ordering,
failed-install retry, recreation after invalidation, late arrivals, wake-clear
races, bounded saturation, and a 10k multi-thread burst. Rust tests assert that
two native clients receive the same handle, initialize one engine, receive
fanout callbacks, and stop receiving callbacks after release. A React Native
0.86.2 consumer build compiles Kotlin and all four Android ABIs from the packed
local package.

The release canary remains one relay-backed Crays Board Orders scenario. It
must complete without `RuntimeTransport::install` SIGSEGV,
`emitRuntimeData`/`CxxCallbackImpl`, or `OutOfMemoryError` before broader QA
resumes.
