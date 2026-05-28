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

The React Native bridge receives native events as:

```ts
{
	v: 1,
	encoding: 'bytes',
	data: number[]
}
```

The JS entry point decodes those events and routes them through the shared
`NativeBackend`. Outgoing `handleMessage` calls pass `number[]` payloads for the
same reason: this legacy React Native native module does not expose a direct
`ArrayBuffer` transport. A future JSI/TurboModule backend should replace the
array bridge with direct `ArrayBuffer`/native-buffer access.

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
