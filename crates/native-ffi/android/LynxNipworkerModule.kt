package com.candypoets.nipworker.lynx

import com.lynx.tasm.behavior.LynxContext
import com.lynx.tasm.module.LynxModule
import com.lynx.tasm.module.LynxMethod
import com.lynx.react.bridge.Callback

/**
 * Lynx native module for NIPWorker.
 *
 * Host app must register this module, e.g.:
 *   LynxViewBuilder.setModule(NipworkerLynxModule::class.java)
 * or via LynxModuleAdapter.
 *
 * The shared library "libnipworker_native_ffi.so" must be bundled in the APK.
 */
class NipworkerLynxModule : LynxModule() {

    companion object {
        init {
            System.loadLibrary("nipworker_native_ffi")
        }

        @JvmStatic
        external fun nipworkerInit(callback: NativeCallback): Long

        @JvmStatic
        external fun nipworkerHandleMessage(handle: Long, bytes: ByteArray)

        @JvmStatic
        external fun nipworkerSetPrivateKey(handle: Long, secret: String)

        @JvmStatic
        external fun nipworkerDeinit(handle: Long)

        @JvmStatic
        external fun nipworkerFreeBytes(ptr: Long, len: Long)
    }

    private var handle: Long = 0

    @LynxMethod
    fun init(callback: Callback) {
        handle = nipworkerInit(object : NativeCallback {
            override fun onData(ptr: Long, len: Int) {
                // In a real implementation, copy from the native pointer into a JVM byte[]
                // and then call nipworkerFreeBytes(ptr, len) to avoid leaking memory.
                // For this skeleton we forward an empty byte array.
                callback.invoke(ByteArray(0))
            }
        })
    }

    @LynxMethod
    fun handleMessage(bytes: ByteArray) {
        if (handle != 0L) {
            nipworkerHandleMessage(handle, bytes)
        }
    }

    @LynxMethod
    fun setPrivateKey(secret: String) {
        if (handle != 0L) {
            nipworkerSetPrivateKey(handle, secret)
        }
    }

    @LynxMethod
    fun deinit() {
        if (handle != 0L) {
            nipworkerDeinit(handle)
            handle = 0
        }
    }

    interface NativeCallback {
        fun onData(ptr: Long, len: Int)
    }
}
