# NIPWorker Native FFI

This crate exposes the Rust `NostrEngine` through a C ABI used by the React Native
and Swift integrations.

## Build Artifacts

- Android: `android/build-android-aar.sh` builds `libnipworker_native_ffi.so`
  for supported ABIs and places them under `android/src/main/jniLibs`.
- iOS/macOS: `ios/build-ios.sh` builds `NipworkerNativeFFI.xcframework` with
  the public C API from `include/nipworker.h`.

The React Native Android bridge consumes the version-matched AAR through its
Prefab package. The Swift package consumes the iOS XCFramework through
`swift/Package.swift`.

Tagged releases publish native binaries to GitHub Releases. Android artifacts
are also published to `https://candypoets.github.io/nipworker/`; pure Android
users can use that hosted Maven repository, the AAR, or the offline Maven archive. Pure Apple
users should use the self-contained Swift SDK archive, or the XCFramework archive
when integrating against the C ABI directly. npm is the distribution point only
for JavaScript and React Native consumers.

## API Ownership

Subscription and publish buffers are owned by Rust. Native hosts should create
subscriptions and publishes through `nipworker_subscribe_message` and
`nipworker_publish_message`, then read buffer pointers with
`nipworker_subscription_buffer_ptr` / `nipworker_subscription_buffer_len`.
