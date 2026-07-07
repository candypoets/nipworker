# NIPWorker React Native Binding

This package scaffold exposes `libnipworker_native_ffi` through a legacy React
Native native module named `NipworkerReactNativeModule`.

## JavaScript

```ts
import { createNostrManager, setManager } from '@candypoets/nipworker/react-native';

const manager = createNostrManager();
setManager(manager);
```

## Event Transport

The React Native entry point installs a JSI byte runtime when possible. Rust owns
subscription and publish buffers; JS receives a small wake event and drains
callback packets from native memory.

Fallback bridge events use:

```ts
{
	v: 1,
	encoding: 'bytes',
	data: number[]
}
```

The fallback path decodes those events in `ReactNativeManager`.

## TurboModule Experiment

This package declares a New Architecture codegen spec at
`src/specs/NativeNipworkerReactNative.ts` and registers it through
`codegenConfig` in `package.json`. The JS entry point prefers the generated
TurboModule when it is available and falls back to the legacy bridge otherwise.

The intended direct-byte shape is a small codegen TurboModule installer plus a
JSI runtime object:

```ts
installByteRuntime(): boolean
globalThis.__nipworkerReactNativeByteRuntime.subscribe(bytes: ArrayBuffer, subId: string): ArrayBuffer
globalThis.__nipworkerReactNativeByteRuntime.publish(bytes: ArrayBuffer, publishId: string): ArrayBuffer
```

The installer keeps the codegen surface on officially supported types while the
JSI runtime carries direct `ArrayBuffer` payloads. Native platform
implementations install that JSI object as
`globalThis.__nipworkerReactNativeByteRuntime`. Rust callback packets are queued
natively, the legacy event emitter sends a tiny `{ encoding: 'queued' }` wake-up,
and JS drains the native queue as `ArrayBuffer[]`. Until installation succeeds,
the JS entry point falls back to the existing `number[]` TurboModule/legacy
bridge shape.

## iOS

Build the existing native FFI framework first:

```sh
crates/native-ffi/ios/build-ios.sh
```

React Native autolinking uses `ios/NipworkerReactNative.podspec`, which vendors
`../../ios/NipworkerNativeFFI.xcframework`.

## Android

The Android module expects `libnipworker_native_ffi.so` to be packaged with the
app for each target ABI. The JNI entry points are exported by
`crates/native-ffi/src/jni.rs` and reuse the existing native implementation in
`crates/native-ffi/android/nipworker_jni_impl.c`.
