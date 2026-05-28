package com.candypoets.nipworker.reactnative

import android.content.Context
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.modules.core.DeviceEventManagerModule
import java.util.concurrent.atomic.AtomicLong

class NipworkerReactNativeModule(
	private val reactContext: ReactApplicationContext
) : ReactContextBaseJavaModule(reactContext) {
	companion object {
		const val NAME = "NipworkerReactNativeModule"
		private const val EVENT_NAME = "NipworkerEvent"
		private const val STORAGE_NAME = "nipworker_storage"

		init {
			System.loadLibrary("nipworker_native_ffi")
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
			val bytes = Arguments.createArray()
			for (byte in data) {
				bytes.pushInt(byte.toInt() and 0xff)
			}
			val payload = Arguments.createMap().apply {
				putInt("v", 1)
				putString("encoding", "bytes")
				putArray("data", bytes)
			}
			module.reactContext
				.getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
				.emit(EVENT_NAME, payload)
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

	override fun getName(): String = NAME

	private val storage by lazy {
		reactContext.getSharedPreferences(STORAGE_NAME, Context.MODE_PRIVATE)
	}

	@ReactMethod
	fun addListener(eventName: String) {
		// Required by NativeEventEmitter on Android. Events are emitted through RCTDeviceEventEmitter.
	}

	@ReactMethod
	fun removeListeners(count: Int) {
		// Required by NativeEventEmitter on Android. Listener cleanup is handled by JavaScript subscriptions.
	}

	@ReactMethod
	fun init() {
		activeModule = this
		if (sharedHandle == 0L) {
			sharedUserdata = nextUserdata.getAndIncrement()
			sharedHandle = nipworkerInit(sharedUserdata)
		}
	}

	@ReactMethod
	fun handleMessage(bytes: ReadableArray) {
		if (sharedHandle != 0L) {
			val data = ByteArray(bytes.size())
			for (i in 0 until bytes.size()) {
				data[i] = (bytes.getInt(i) and 0xff).toByte()
			}
			nipworkerHandleMessage(sharedHandle, data)
		}
	}

	@ReactMethod
	fun setPrivateKey(secret: String) {
		if (sharedHandle != 0L) {
			nipworkerSetPrivateKey(sharedHandle, secret)
		}
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	fun getStorageItem(key: String): String? {
		return storage.getString(key, null)
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	fun setStorageItem(key: String, value: String): Boolean {
		return storage.edit().putString(key, value).commit()
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	fun removeStorageItem(key: String): Boolean {
		return storage.edit().remove(key).commit()
	}

	@ReactMethod
	fun deinit() {
		if (sharedHandle != 0L) {
			nipworkerDeinit(sharedHandle)
			sharedHandle = 0L
			sharedUserdata = 0L
		}
		if (activeModule === this) {
			activeModule = null
		}
	}
}
