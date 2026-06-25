import { LynxContext, LynxModule } from '@lynx/lynx';

/**
 * Lynx native module for HarmonyOS (ArkTS).
 *
 * This module loads the NAPI C++ addon (`libnipworker_napi.so`) which
 * bridges to the Rust `libnipworker_native_ffi.so`.
 *
 * Registration:
 *   this.modules.set('NipworkerLynxModule', { moduleClass: NipworkerLynxModule })
 *
 * Build steps:
 *   1. Place `nipworker_napi.cpp` in the `cpp/` directory of your HarmonyOS module.
 *   2. Reference it in `build-profile.json5` under `nativeLib` or `cpp`.
 *   3. Ensure `libnipworker_native_ffi.so` is bundled in the HAP.
 */

// NAPI function signatures (declared in the C++ addon)
declare function nipworkerInit(callback: (data: ArrayBuffer) => void): bigint;
declare function nipworkerHandleMessage(handle: bigint, bytes: ArrayBuffer): void;
declare function nipworkerSetPrivateKey(handle: bigint, secret: string): void;
declare function nipworkerDeinit(handle: bigint): void;
declare function nipworkerFreeBytes(ptr: bigint, len: bigint): void;

export class NipworkerLynxModule extends LynxModule {
  private handle: bigint = 0n;

  constructor(context: LynxContext) {
    super(context);
  }

  /**
   * Initialise the native engine.
   * The Rust engine runs in a background thread and forwards events
   * back to ArkTS via the NAPI threadsafe function.
   */
  init(callback: (data: ArrayBuffer) => void): void {
    this.handle = nipworkerInit(callback);
  }

  /**
   * Pass a FlatBuffers message to the native engine.
   */
  handleMessage(bytes: ArrayBuffer): void {
    if (this.handle !== 0n) {
      nipworkerHandleMessage(this.handle, bytes);
    }
  }

  /**
   * Set a raw private key on the native signer.
   */
  setPrivateKey(secret: string): void {
    if (this.handle !== 0n) {
      nipworkerSetPrivateKey(this.handle, secret);
    }
  }

  /**
   * Tear down the native engine.
   */
  deinit(): void {
    if (this.handle !== 0n) {
      nipworkerDeinit(this.handle);
      this.handle = 0n;
    }
  }
}
