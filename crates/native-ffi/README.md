# NIPWorker Native FFI

> **Status: untested skeletons.**  
> The iOS, Android, and HarmonyOS wrappers below have not been compiled or run in a real Lynx host app. They are provided as a starting point for mobile integrators.

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

## Platform wrappers

### iOS (`ios/LynxNipworkerModule.mm`)

* **Language:** Objective-C++
* **Registration:** `[globalConfig registerModule:NipworkerLynxModule.class];`
* **Linking:** Link `libnipworker_native_ffi.a` (static library) built from this crate.
* **Build steps (typical):**
  1. `cargo build --release --target aarch64-apple-ios` (and `x86_64-apple-ios` for simulators)
  2. `lipo` or `xcframework` the resulting static library.
  3. Add the `.a` and the `.mm` file to the host Xcode project.

### Android (`android/LynxNipworkerModule.kt`)

* **Language:** Kotlin
* **Registration:** via `LynxViewBuilder.setModule(NipworkerLynxModule::class.java)` or your `LynxModuleAdapter`.
* **Linking:** Bundle `libnipworker_native_ffi.so` for each target ABI.
* **JNI Bridge:** The Kotlin `external` declarations expect a thin JNI C layer that translates between JNI types and the Rust C API above, and forwards callbacks back to Kotlin via `onNativeData()`.
* **Build steps (typical):**
  1. Use `cargo-ndk` or a custom CMake/ndk-build step:
     ```bash
     cargo ndk -t armeabi-v7a -t arm64-v8a -t x86 -t x86_64 build --release
     ```
  2. Copy the resulting `.so` files into `jniLibs/<abi>/`.
  3. Write a JNI C bridge (e.g. `nipworker_jni.c`) that wraps the 5 C functions and manages global references for callbacks.
  4. Include `LynxNipworkerModule.kt` in the app module sources.

### HarmonyOS (`harmony/LynxNipworkerModule.ets`)

* **Language:** ArkTS
* **Registration:** `this.modules.set('NipworkerLynxModule', { moduleClass: NipworkerLynxModule })`
* **Linking:** Not yet implemented. ArkTS cannot call C directly without a NAPI/FFI bridge.
* **Build steps (future):**
  1. Write a NAPI C++ addon that wraps the 5 C functions above.
  2. Build the addon into an `.so` shipped with the HarmonyOS app.
  3. Replace the `TODO` stubs in `LynxNipworkerModule.ets` with actual NAPI calls.

## TypeScript usage

```ts
import { NativeBackend, setManager } from '@candypoets/nipworker';

const backend = new NativeBackend();
setManager(backend);
```

`NativeBackend` automatically looks for the Lynx native module under:

* `globalThis.lynx.getNativeModules().NipworkerLynxModule`
* `globalThis.NativeModules.NipworkerLynxModule`

## Known limitations

* NIP-07 (browser extension) is not applicable in native mobile contexts and will warn at runtime.
* NIP-46 (remote signer) is declared as a skeleton; full proxy-signer callbacks from the Rust engine are not yet wired through the C FFI.
* Wake/visibility messages are not yet forwarded to the native engine.
