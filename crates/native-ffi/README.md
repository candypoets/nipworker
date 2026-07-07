# NIPWorker Native FFI

This crate exposes the Rust `NostrEngine` through a C ABI used by the React Native
and Swift integrations.

## Build Artifacts

- Android: `android/build-android-aar.sh` builds `libnipworker_native_ffi.so`
  for supported ABIs and places them under `android/src/main/jniLibs`.
- iOS/macOS: `ios/build-ios.sh` builds `NipworkerNativeFFI.xcframework`.

The React Native package consumes these artifacts from
`crates/native-ffi/react-native`. The Swift package consumes the iOS
XCFramework through `swift/Package.swift`.

## API Ownership

Subscription and publish buffers are owned by Rust. Native hosts should create
subscriptions and publishes through `nipworker_subscribe_message` and
`nipworker_publish_message`, then read buffer pointers with
`nipworker_subscription_buffer_ptr` / `nipworker_subscription_buffer_len`.
