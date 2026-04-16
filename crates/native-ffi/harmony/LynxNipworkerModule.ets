import { LynxContext, LynxModule } from '@lynx/lynx';

/**
 * Lynx native module stub for HarmonyOS (ArkTS).
 *
 * ArkTS cannot directly invoke C functions without a NAPI/FFI bridging layer.
 * Each method below contains a TODO explaining the required native integration.
 *
 * Registration:
 *   this.modules.set('NipworkerLynxModule', { moduleClass: NipworkerLynxModule })
 */
export class NipworkerLynxModule extends LynxModule {
  constructor(context: LynxContext) {
    super(context);
  }

  /**
   * Initialise the native engine.
   * TODO: Implement a NAPI C++ addon that calls nipworker_init(callbackForwarder, userdata)
   * and forwards the Rust callback back to this ArkTS callback via an ArrayBuffer.
   */
  init(callback: (data: ArrayBuffer) => void): void {
    // TODO: wire through NAPI/FFI to nipworker_init(callbackForwarder)
    console.warn('[NipworkerLynxModule] init() is a skeleton on HarmonyOS');
  }

  /**
   * Pass a FlatBuffers message to the native engine.
   * TODO: Forward the bytes to the NAPI layer which calls nipworker_handle_message().
   */
  handleMessage(bytes: ArrayBuffer): void {
    // TODO: wire through NAPI/FFI to nipworker_handle_message(handle, ptr, len)
    console.warn('[NipworkerLynxModule] handleMessage() is a skeleton on HarmonyOS');
  }

  /**
   * Set a raw private key on the native signer.
   * TODO: Forward the secret string to the NAPI layer which calls
   * nipworker_set_private_key(handle, secret).
   */
  setPrivateKey(secret: string): void {
    // TODO: wire through NAPI/FFI to nipworker_set_private_key(handle, secret)
    console.warn('[NipworkerLynxModule] setPrivateKey() is a skeleton on HarmonyOS');
  }

  /**
   * Tear down the native engine.
   * TODO: Call nipworker_deinit(handle) from the NAPI layer.
   */
  deinit(): void {
    // TODO: wire through NAPI/FFI to nipworker_deinit(handle)
    console.warn('[NipworkerLynxModule] deinit() is a skeleton on HarmonyOS');
  }
}
