package com.candypoets.nipworker.lynx

import com.lynx.tasm.behavior.LynxContext
import com.lynx.tasm.module.LynxModule
import com.lynx.tasm.module.LynxMethod
import com.lynx.react.bridge.Callback
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicLong

/**
 * Lynx native module for NIPWorker.
 *
 * Host app must register this module, e.g.:
 *   LynxViewBuilder.setModule(NipworkerLynxModule::class.java)
 * or via LynxModuleAdapter.
 *
 * The shared library "libnipworker_native_ffi.so" must be bundled in the APK.
 *
 * NOTE: These external declarations target a thin JNI C bridge that translates
 * between JNI and the Rust C ABI. The bridge is responsible for forwarding
 * Rust callbacks back to Kotlin via [onNativeData].
 */
class NipworkerLynxModule : LynxModule() {

    companion object {
        init {
            System.loadLibrary("nipworker_native_ffi")
        }

        private val callbacks = ConcurrentHashMap<Long, Callback>()
        private val nextUserdata = AtomicLong(1L)

        /**
         * Invoked by the JNI C bridge when the Rust engine emits data.
         * The [userdata] value is the one originally passed to [nipworkerInit].
         */
        @JvmStatic
        fun onNativeData(userdata: Long, data: ByteArray) {
            callbacks[userdata]?.invoke(data)
        }

        @JvmStatic
        external fun nipworkerInit(userdata: Long): Long

        @JvmStatic
        external fun nipworkerHandleMessage(handle: Long, bytes: ByteArray)

        @JvmStatic
        external fun nipworkerSetPrivateKey(handle: Long, secret: String)

        @JvmStatic
        external fun nipworkerDeinit(handle: Long)

        @JvmStatic
        external fun nipworkerFreeBytes(ptr: Long, len: Long)
    }

    private var handle: Long = 0L
    private var userdata: Long = 0L

    @LynxMethod
    fun init(callback: Callback) {
        userdata = nextUserdata.getAndIncrement()
        handle = nipworkerInit(userdata)
        if (handle != 0L) {
            callbacks[userdata] = callback
        }
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
            callbacks.remove(userdata)
            nipworkerDeinit(handle)
            handle = 0
            userdata = 0
        }
    }
}
