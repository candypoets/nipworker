# Nipworker iOS Native Module

## Prerequisites

- macOS with Xcode 15+
- Rust toolchain with Apple targets:
  ```bash
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim x86_64-apple-ios
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  ```

## Build

From this directory, run the build script:

```bash
./build-ios.sh
```

This script:
1. Builds `libnipworker_native_ffi.a` for iOS device, iOS simulator, and macOS
2. Creates universal simulator and macOS binaries with `lipo`
3. Creates `NipworkerNativeFFI.xcframework`

By default, the script builds with `IPHONEOS_DEPLOYMENT_TARGET=14.0` and
`MACOSX_DEPLOYMENT_TARGET=11.0`. Override either environment variable before
running the script if the consuming package raises its minimum supported OS.

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
