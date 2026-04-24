# Nipworker iOS Native Module

## Prerequisites

- macOS with Xcode 15+
- Rust toolchain with iOS targets:
  ```bash
  rustup target add aarch64-apple-ios x86_64-apple-ios aarch64-apple-ios-sim
  ```

## Build the Rust static library

From the `crates/native-ffi` directory:

```bash
# Device (arm64)
cargo build --release --target aarch64-apple-ios

# Simulator (Intel)
cargo build --release --target x86_64-apple-ios

# Simulator (Apple Silicon)
cargo build --release --target aarch64-apple-ios-sim
```

## Create a universal binary

```bash
lipo -create \
  target/aarch64-apple-ios/release/libnipworker_native_ffi.a \
  target/x86_64-apple-ios/release/libnipworker_native_ffi.a \
  target/aarch64-apple-ios-sim/release/libnipworker_native_ffi.a \
  -output ios/libnipworker_native_ffi.a
```

> **Note:** If you only need device + Apple Silicon simulator, omit the `x86_64` slice.

## CocoaPods integration

The `Nipworker.podspec` in this directory references `libnipworker_native_ffi.a` and `LynxNipworkerModule.mm`.

In your app's `Podfile`:
```ruby
pod 'Nipworker', :path => '../node_modules/@candypoets/nipworker/crates/native-ffi/ios'
```

Then run:
```bash
cd ios && pod install
```

## Registration

The module is automatically registered by CocoaPods via the `LynxModule` protocol. No manual `[LynxEnv registerModule:]` call is required unless you prefer to do it in your `AppDelegate`.
