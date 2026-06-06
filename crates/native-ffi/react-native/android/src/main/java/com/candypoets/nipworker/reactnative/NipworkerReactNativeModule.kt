package com.candypoets.nipworker.reactnative

import android.content.Context
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.modules.core.DeviceEventManagerModule
import java.util.concurrent.atomic.AtomicLong

@ReactModule(name = NipworkerReactNativeModule.NAME)
class NipworkerReactNativeModule(
	private val reactContext: ReactApplicationContext
) : NativeNipworkerReactNativeSpec(reactContext) {
	companion object {
		const val NAME = "NipworkerReactNativeModule"
		private const val EVENT_NAME = "NipworkerEvent"
		private const val STORAGE_NAME = "nipworker_storage"

		init {
			System.loadLibrary("nipworker_native_ffi")
			System.loadLibrary("nipworker_react_native")
		}

		private val nextUserdata = AtomicLong(1L)

		@Volatile
		private var sharedHandle: Long = 0L

		@Volatile
		private var sharedUserdata: Long = 0L

		@Volatile
		private var activeModule: NipworkerReactNativeModule? = null

		@JvmStatic
		fun onNativeData(userdata: Long, data: ByteArray) {
			val module = activeModule ?: return
			if (nativeIsByteRuntimeInstalled() && nativeQueueData(data)) {
				val payload = Arguments.createMap().apply {
					putInt("v", 1)
					putString("encoding", "queued")
				}
				module.emitData(payload)
				return
			}
			val bytes = Arguments.createArray()
			for (byte in data) {
				bytes.pushInt(byte.toInt() and 0xff)
			}
			val payload = Arguments.createMap().apply {
				putInt("v", 1)
				putString("encoding", "bytes")
				putArray("data", bytes)
			}
			module.emitData(payload)
		}

		@JvmStatic
		external fun nipworkerInit(userdata: Long): Long

		@JvmStatic
		external fun nipworkerInitWithStoragePath(userdata: Long, storagePath: String): Long

		@JvmStatic
		external fun nipworkerHandleMessage(handle: Long, bytes: ByteArray)

		@JvmStatic
		external fun nipworkerSetPrivateKey(handle: Long, secret: String)

		@JvmStatic
		external fun nipworkerDeinit(handle: Long)

		@JvmStatic
		external fun nipworkerFreeBytes(ptr: Long, len: Long)

		@JvmStatic
		external fun nativeInstallByteRuntime(runtimePtr: Long, handle: Long): Boolean

		@JvmStatic
		external fun nativeIsByteRuntimeInstalled(): Boolean

		@JvmStatic
		external fun nativeQueueData(bytes: ByteArray): Boolean
	}

	private val storage by lazy {
		reactContext.getSharedPreferences(STORAGE_NAME, Context.MODE_PRIVATE)
	}

	@ReactMethod
	fun addListener(eventName: String) {
		// Required by NativeEventEmitter on Android legacy paths.
	}

	@ReactMethod
	fun removeListeners(count: Int) {
		// Required by NativeEventEmitter on Android legacy paths.
	}

	override fun initEngine() {
		activeModule = this
		if (sharedHandle == 0L) {
			sharedUserdata = nextUserdata.getAndIncrement()
			val cacheDir = reactContext.filesDir.resolve("nipworker")
			sharedHandle = nipworkerInitWithStoragePath(sharedUserdata, cacheDir.absolutePath)
		}
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun installByteRuntime(): Boolean {
		initEngine()
		val runtimePtr = reactContext.javaScriptContextHolder?.get() ?: 0L
		if (runtimePtr == 0L || sharedHandle == 0L) {
			return false
		}
		return nativeInstallByteRuntime(runtimePtr, sharedHandle)
	}

	@ReactMethod
	override fun handleMessage(bytes: ReadableArray) {
		if (sharedHandle != 0L) {
			val data = ByteArray(bytes.size())
			for (i in 0 until bytes.size()) {
				data[i] = (bytes.getInt(i) and 0xff).toByte()
			}
			nipworkerHandleMessage(sharedHandle, data)
		}
	}

	@ReactMethod
	override fun setPrivateKey(secret: String) {
		if (sharedHandle != 0L) {
			nipworkerSetPrivateKey(sharedHandle, secret)
		}
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun getStorageItem(key: String): String? {
		return storage.getString(key, null)
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun setStorageItem(key: String, value: String): Boolean {
		return storage.edit().putString(key, value).commit()
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun removeStorageItem(key: String): Boolean {
		return storage.edit().remove(key).commit()
	}

	override fun deinitEngine() {
		if (sharedHandle != 0L) {
			nipworkerDeinit(sharedHandle)
			sharedHandle = 0L
			sharedUserdata = 0L
		}
		if (activeModule === this) {
			activeModule = null
		}
	}

	private fun emitData(payload: com.facebook.react.bridge.WritableMap) {
		try {
			emitOnData(payload)
		} catch (_: Throwable) {
			reactContext
				.getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
				.emit(EVENT_NAME, payload)
		}
	}
}
