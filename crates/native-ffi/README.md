# NIPWorker Native FFI

> **Status: Android and iOS builds verified. HarmonyOS is a skeleton.**
> The TypeScript `createNostrManager()` auto-detects LynxJS and returns `NativeBackend` automatically.

## Overview

`crates/native-ffi` exposes a C API over the Rust `NostrEngine` so that native mobile applications can reuse the same core logic as the WASM/TypeScript builds.

```c
void* nipworker_init(void (*callback)(void* userdata, const uint8_t* ptr, size_t len), void* userdata);
void nipworker_handle_message(void* handle, const uint8_t* ptr, size_t len);
void nipworker_set_private_key(void* handle, const char* ptr);
void nipworker_deinit(void* handle);
void nipworker_free_bytes(uint8_t* ptr, size_t len);
```

The TypeScript side provides `NativeBackend` (`src/NativeBackend.ts`) which implements the same public interface as `EngineManager` / `NostrManager` and communicates with the native code through a Lynx module named `NipworkerLynxModule`.

## Quick Start for LynxJS Developers

```typescript
import { createNostrManager, setManager } from '@candypoets/nipworker';

// Auto-detects Lynx native module, WASM engine, or legacy 4-worker
const backend = createNostrManager();
setManager(backend);
```

`createNostrManager()` detects the runtime in this order:
1. `globalThis.lynx.getNativeModules().NipworkerLynxModule` exists → `NativeBackend`
2. `config.engine === true` → `EngineManager` (single WASM worker)
3. Otherwise → `NostrManager` (legacy 4-worker WASM)

## Pre-built Binaries

Every git tag `v*` triggers a [GitHub Actions workflow](../../.github/workflows/native-build.yml) that builds and attaches native libraries to the release:

| Platform | Artifact | Download |
|----------|----------|----------|
| **Android** | `nipworker-native-android.zip` | GitHub Release attachments |
| **iOS** | `nipworker-native-ios.zip` (XCFramework) | GitHub Release attachments |
| **Linux** | `nipworker-native-linux.zip` | GitHub Release attachments |

## Platform Integration

### Android

**Files you need:**
- `android/LynxNipworkerModule.kt` — Kotlin Lynx module
- `libnipworker_native_ffi.so` — built from this crate (all 4 ABIs)

**Integration steps:**
1. Download `nipworker-native-android.zip` from the GitHub Release (or build locally with `cargo-ndk`).
2. Unzip to `android/app/src/main/jniLibs/`:
   ```
   jniLibs/
   ├── arm64-v8a/libnipworker_native_ffi.so
   ├── armeabi-v7a/libnipworker_native_ffi.so
   ├── x86/libnipworker_native_ffi.so
   └── x86_64/libnipworker_native_ffi.so
   ```
3. Copy `crates/native-ffi/android/LynxNipworkerModule.kt` into your app module sources.
4. Register the module in your Lynx setup:
   ```kotlin
   LynxViewBuilder.setModule(NipworkerLynxModule::class.java)
   ```

**Local build (requires Android NDK):**
```bash
cd crates/native-ffi
cargo ndk -t armeabi-v7a -t arm64-v8a -t x86 -t x86_64 build --release
```

### iOS

**Files you need:**
- `ios/LynxNipworkerModule.mm` — Objective-C++ Lynx module
- `NipworkerNativeFFI.xcframework` — built from this crate

**Integration steps:**
1. Download `nipworker-native-ios.zip` from the GitHub Release (or build locally on macOS).
2. Drag `NipworkerNativeFFI.xcframework` into your Xcode project.
3. In **Frameworks, Libraries, and Embedded Content**, set it to **Embed & Sign**.
4. Copy `crates/native-ffi/ios/LynxNipworkerModule.mm` into your Xcode project.
5. Register the module:
   ```objc
   [globalConfig registerModule:NipworkerLynxModule.class];
   ```

**Local build (requires macOS + Xcode):**
```bash
cd crates/native-ffi
cargo build --release --target aarch64-apple-ios
cargo build --release --target aarch64-apple-ios-sim
cargo build --release --target x86_64-apple-ios

# Create XCFramework
lipo -create \
  target/aarch64-apple-ios-sim/release/libnipworker_native_ffi.a \
  target/x86_64-apple-ios/release/libnipworker_native_ffi.a \
  -output libnipworker_native_ffi_sim.a

xcodebuild -create-xcframework \
  -library target/aarch64-apple-ios/release/libnipworker_native_ffi.a \
  -library libnipworker_native_ffi_sim.a \
  -output NipworkerNativeFFI.xcframework
```

### HarmonyOS

* **Language:** ArkTS
* **Registration:** `this.modules.set('NipworkerLynxModule', { moduleClass: NipworkerLynxModule })`
* **Linking:** Not yet implemented. ArkTS cannot call C directly without a NAPI/FFI bridge.
* **Build steps (future):**
  1. Write a NAPI C++ addon that wraps the 5 C functions above.
  2. Build the addon into an `.so` shipped with the HarmonyOS app.
  3. Replace the `TODO` stubs in `LynxNipworkerModule.ets` with actual NAPI calls.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  TypeScript App (LynxJS)                                    │
│  createNostrManager() → NativeBackend                       │
│         │                                                   │
│         ▼                                                   │
│  ┌─────────────────────────────────────┐                   │
│  │ NativeBackend.ts                    │                   │
│  │  calls lynx.getNativeModules()...   │                   │
│  └─────────────────────────────────────┘                   │
│         │                                                   │
│         ▼                                                   │
│  ┌─────────────────────────────────────┐                   │
│  │ Platform wrapper                    │                   │
│  │  Android: Kotlin + JNI C bridge     │                   │
│  │  iOS:     Objective-C++             │                   │
│  └─────────────────────────────────────┘                   │
│         │                                                   │
│         ▼                                                   │
│  ┌─────────────────────────────────────┐                   │
│  │ Rust: libnipworker_native_ffi.so/.a │                   │
│  │  C ABI → NostrEngine orchestrator   │                   │
│  └─────────────────────────────────────┘                   │
└─────────────────────────────────────────────────────────────┘
```

## Known limitations

* NIP-07 (browser extension) is not applicable in native mobile contexts and will warn at runtime.
* NIP-46 (remote signer) is declared as a skeleton; full proxy-signer callbacks from the Rust engine are not yet wired through the C FFI.
* Wake/visibility messages are not yet forwarded to the native engine.
