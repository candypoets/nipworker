# Nipworker iOS Native Module

## Prerequisites

- macOS with Xcode 15+
- Rust toolchain with iOS targets:
  ```bash
  rustup target add aarch64-apple-ios x86_64-apple-ios aarch64-apple-ios-sim
  ```

## Build

From this directory, run the build script:

```bash
./build-ios.sh
```

This script:
1. Builds `libnipworker_native_ffi.a` for device (arm64) and simulator (arm64 + x86_64)
2. Creates a universal fat binary with `lipo`
3. Creates `NipworkerNativeFFI.xcframework`

## CocoaPods integration

The `Nipworker.podspec` in this directory references `libnipworker_native_ffi.a`.
Make sure you ran `./build-ios.sh` first so the static library exists.

In your app's `Podfile`:
```ruby
pod 'Nipworker', :path => '../node_modules/@candypoets/nipworker/crates/native-ffi/ios'
```

Then run:
```bash
cd ios && pod install
```

## Manual Xcode integration (no CocoaPods)

Drag `NipworkerNativeFFI.xcframework` into your Xcode project.
In **Frameworks, Libraries, and Embedded Content**, set it to **Embed & Sign**.
Also add `LynxNipworkerModule.h` and `LynxNipworkerModule.mm` to your build target.

## Registration

The module is automatically registered by CocoaPods via the `LynxModule` protocol. No manual `[LynxEnv registerModule:]` call is required unless you prefer to do it in your `AppDelegate`.

```objc
[globalConfig registerModule:NipworkerLynxModule.class];
```
