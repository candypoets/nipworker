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
| **Android** | `nipworker-native-ffi-android-release.aar` | GitHub Release attachments |
| **iOS** | `nipworker-native-ios.zip` (XCFramework) | GitHub Release attachments |
| **Linux** | `nipworker-native-linux.zip` | GitHub Release attachments |

## Platform Integration

### Android

The Android artifact is a first-class AAR that contains:
- `com.candypoets.nipworker.lynx.NipworkerLynxModule`
- `libnipworker_native_ffi.so` for `arm64-v8a`, `armeabi-v7a`, `x86`, and `x86_64`
- consumer ProGuard/R8 keep rules for the native module and JNI methods

**Maven integration:**
```kotlin
implementation("com.candypoets:nipworker-native-ffi-android:0.96.0")
```

Register the module in your Sparkling/Lynx setup:
```kotlin
"NipworkerLynxModule" to SparklingLynxModuleWrapper(
    NipworkerLynxModule::class.java,
    null
)
```

The AAR declares Lynx/Sparkling APIs as compile-only. The host app is expected to provide the real Lynx runtime.

**Local monorepo / `node_modules` fallback:**
```kotlin
include(":nipworker-native-ffi-android")
project(":nipworker-native-ffi-android").projectDir =
    file("../node_modules/@candypoets/nipworker/crates/native-ffi/android")
```

Then depend on it from the app:
```kotlin
implementation(project(":nipworker-native-ffi-android"))
```

**Local build (requires Android NDK):**
```bash
cd crates/native-ffi/android
./build-android-aar.sh
./validate-aar.sh
```

`build-android-aar.sh` builds Rust with `cargo-ndk --release`, copies all four ABI outputs into `src/main/jniLibs`, strips release symbols when `llvm-strip` is available, and runs `assembleRelease`.

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

**Local build (requires macOS + Xcode + Rust):**
```bash
cd crates/native-ffi/ios
./build-ios.sh
```

This produces:
- `ios/libnipworker_native_ffi.a` — universal static library for CocoaPods
- `ios/NipworkerNativeFFI.xcframework` — modern XCFramework for manual Xcode integration

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
