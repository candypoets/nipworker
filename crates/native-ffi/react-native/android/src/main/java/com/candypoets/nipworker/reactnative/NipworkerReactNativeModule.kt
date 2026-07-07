package com.candypoets.nipworker.reactnative

import android.content.Context
import com.facebook.react.bridge.Arguments
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactContextBaseJavaModule
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.modules.core.DeviceEventManagerModule
import com.facebook.react.turbomodule.core.interfaces.TurboModule
import java.nio.ByteBuffer
import java.nio.ByteOrder
import java.util.concurrent.atomic.AtomicLong
import nostr.fb.WorkerMessage

typealias NipworkerRuntimeListener = (ByteArray) -> Unit

object NipworkerRuntime {
	private const val STORAGE_NAME = "nipworker_storage"
	private val nextUserdata = AtomicLong(1L)
	private val listeners = LinkedHashSet<NipworkerRuntimeListener>()
	private val keyedListeners = LinkedHashMap<String, LinkedHashSet<NipworkerRuntimeListener>>()

	@Volatile
	private var sharedHandle: Long = 0L

	@Volatile
	private var sharedUserdata: Long = 0L

	val handle: Long
		get() = sharedHandle

	fun addListener(listener: NipworkerRuntimeListener): () -> Unit {
		synchronized(this) {
			listeners.add(listener)
		}
		return {
			synchronized(this) {
				listeners.remove(listener)
			}
		}
	}

	fun addListener(id: String, listener: NipworkerRuntimeListener): () -> Unit {
		synchronized(this) {
			keyedListeners.getOrPut(id) { LinkedHashSet() }.add(listener)
		}
		return {
			synchronized(this) {
				val listenersForId = keyedListeners[id]
				if (listenersForId != null) {
					listenersForId.remove(listener)
					if (listenersForId.isEmpty()) {
						keyedListeners.remove(id)
					}
				}
			}
		}
	}

	fun init(context: Context, defaultRelays: ReadableArray? = null, indexerRelays: ReadableArray? = null): Long {
		synchronized(this) {
			if (sharedHandle == 0L) {
				sharedUserdata = nextUserdata.getAndIncrement()
				val cacheDir = context.filesDir.resolve("nipworker")
				sharedHandle = NipworkerReactNativeModule.nipworkerInitWithConfig(
					sharedUserdata,
					cacheDir.absolutePath,
					readableArrayToCsv(defaultRelays),
					readableArrayToCsv(indexerRelays)
				)
			}
			return sharedHandle
		}
	}

	fun handleMessage(bytes: ByteArray) {
		val handle = sharedHandle
		if (handle != 0L) {
			NipworkerReactNativeModule.nipworkerHandleMessage(handle, bytes)
		}
	}

	fun subscribe(message: ByteArray, subId: String): ByteBuffer? {
		val handle = sharedHandle
		if (handle == 0L) return null
		retainSubscriptionBuffer(subId)?.let { return it }
		return NipworkerReactNativeModule.nativeSubscribeMessage(handle, message, subId)
	}

	fun publish(message: ByteArray, publishId: String): ByteBuffer? {
		val handle = sharedHandle
		return if (handle == 0L) null else NipworkerReactNativeModule.nativePublishMessage(handle, message, publishId)
	}

	fun setPrivateKey(secret: String) {
		val handle = sharedHandle
		if (handle != 0L) {
			NipworkerReactNativeModule.nipworkerSetPrivateKey(handle, secret)
		}
	}

	fun wake() {
		val handle = sharedHandle
		if (handle != 0L) {
			NipworkerReactNativeModule.nipworkerWake(handle)
		}
	}

	fun registerSubscription(subId: String, bufferSize: Int): Boolean {
		val handle = sharedHandle
		return handle != 0L && NipworkerReactNativeModule.nativeRegisterSubscription(handle, subId, bufferSize)
	}

	fun registerPublishBuffer(publishId: String, bufferSize: Int): Boolean {
		val handle = sharedHandle
		return handle != 0L && NipworkerReactNativeModule.nativeRegisterPublishBuffer(handle, publishId, bufferSize)
	}

	fun retainSubscription(subId: String): Boolean {
		val handle = sharedHandle
		return handle != 0L && NipworkerReactNativeModule.nativeRetainSubscription(handle, subId)
	}

	fun retainSubscriptionBuffer(subId: String): ByteBuffer? {
		val handle = sharedHandle
		if (handle == 0L || !NipworkerReactNativeModule.nativeRetainSubscription(handle, subId)) {
			return null
		}
		return NipworkerReactNativeModule.nativeGetSubscriptionBuffer(handle, subId)
			?: run {
				NipworkerReactNativeModule.nativeReleaseSubscription(handle, subId)
				null
			}
	}

	fun releaseSubscription(subId: String) {
		val handle = sharedHandle
		if (handle != 0L) {
			NipworkerReactNativeModule.nativeReleaseSubscription(handle, subId)
		}
	}

	fun subscriptionBuffer(subId: String): ByteBuffer? {
		val handle = sharedHandle
		return if (handle == 0L) null else NipworkerReactNativeModule.nativeGetSubscriptionBuffer(handle, subId)
	}

	fun deinit() {
		synchronized(this) {
			if (sharedHandle != 0L) {
				NipworkerReactNativeModule.nipworkerDeinit(sharedHandle)
				sharedHandle = 0L
				sharedUserdata = 0L
			}
			listeners.clear()
			keyedListeners.clear()
		}
	}

	internal fun dispatch(data: ByteArray) {
		val subId = readSubId(data)
		val (globalSnapshot, keyedSnapshot) = synchronized(this) {
			listeners.toList() to if (subId == null) {
				emptyList<NipworkerRuntimeListener>()
			} else {
				keyedListeners[subId].orEmpty().toList()
			}
		}
		for (listener in globalSnapshot) {
			listener(data)
		}
		for (listener in keyedSnapshot) {
			listener(data)
		}
	}

	private fun readSubId(data: ByteArray): String? {
		decodeRouteWakeFrame(data)?.let { return it }
		return runCatching {
			val buffer = ByteBuffer.wrap(data).order(ByteOrder.LITTLE_ENDIAN)
			WorkerMessage.getRootAsWorkerMessage(buffer).subId()
		}.getOrNull()
	}

	private fun decodeRouteWakeFrame(data: ByteArray): String? {
		if (data.size < 8) return null
		if (
			data[0] != 0x4e.toByte() ||
			data[1] != 0x57.toByte() ||
			data[2] != 0x52.toByte() ||
			data[3] != 0x31.toByte()
		) {
			return null
		}
		val subIdLength = ByteBuffer.wrap(data, 4, 4).order(ByteOrder.LITTLE_ENDIAN).int
		if (subIdLength <= 0 || subIdLength != data.size - 8) return null
		return data.decodeToString(8, data.size)
	}

	private fun readableArrayToCsv(values: ReadableArray?): String {
		if (values == null) {
			return ""
		}
		val relays = mutableListOf<String>()
		for (i in 0 until values.size()) {
			val relay = values.getString(i)?.trim()
			if (!relay.isNullOrEmpty() && !relay.contains(",")) {
				relays.add(relay)
			}
		}
		return relays.joinToString(",")
	}
}

@ReactModule(name = NipworkerReactNativeModule.NAME)
class NipworkerReactNativeModule(
	private val reactContext: ReactApplicationContext
) : ReactContextBaseJavaModule(reactContext), TurboModule {
	companion object {
		const val NAME = "NipworkerReactNativeModule"
		private const val EVENT_NAME = "NipworkerEvent"
		private const val STORAGE_NAME = "nipworker_storage"

		init {
			System.loadLibrary("nipworker_native_ffi")
			System.loadLibrary("nipworker_react_native")
		}

		@JvmStatic
		fun onNativeData(userdata: Long, data: ByteArray) {
			NipworkerRuntime.dispatch(data)
		}

		@JvmStatic
		external fun nipworkerInit(userdata: Long): Long

		@JvmStatic
		external fun nipworkerInitWithStoragePath(userdata: Long, storagePath: String): Long

		@JvmStatic
		external fun nipworkerInitWithConfig(
			userdata: Long,
			storagePath: String,
			defaultRelays: String,
			indexerRelays: String
		): Long

		@JvmStatic
		external fun nipworkerHandleMessage(handle: Long, bytes: ByteArray)

		@JvmStatic
		external fun nipworkerSetPrivateKey(handle: Long, secret: String)

		@JvmStatic
		external fun nipworkerWake(handle: Long)

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

		@JvmStatic
		external fun nativeSubscribeMessage(handle: Long, bytes: ByteArray, subId: String): ByteBuffer?

		@JvmStatic
		external fun nativePublishMessage(handle: Long, bytes: ByteArray, publishId: String): ByteBuffer?

		@JvmStatic
		external fun nativeRegisterSubscription(handle: Long, subId: String, bufferSize: Int): Boolean

		@JvmStatic
		external fun nativeRegisterPublishBuffer(handle: Long, publishId: String, bufferSize: Int): Boolean

		@JvmStatic
		external fun nativeRetainSubscription(handle: Long, subId: String): Boolean

		@JvmStatic
		external fun nativeReleaseSubscription(handle: Long, subId: String)

		@JvmStatic
		external fun nativeGetSubscriptionBuffer(handle: Long, subId: String): ByteBuffer?
	}

	private var removeRuntimeListener: (() -> Unit)? = null

	override fun getName(): String = NAME

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

	@ReactMethod
	fun initEngine(defaultRelays: ReadableArray, indexerRelays: ReadableArray) {
		ensureRuntimeListener()
		NipworkerRuntime.init(reactContext, defaultRelays, indexerRelays)
	}

	private fun ensureRuntimeListener() {
		if (removeRuntimeListener == null) {
			removeRuntimeListener = NipworkerRuntime.addListener { data ->
				emitRuntimeData(data)
			}
		}
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	fun installByteRuntime(): Boolean {
		initEngine(Arguments.createArray(), Arguments.createArray())
		val runtimePtr = reactContext.javaScriptContextHolder?.get() ?: 0L
		val handle = NipworkerRuntime.handle
		if (runtimePtr == 0L || handle == 0L) {
			return false
		}
		return nativeInstallByteRuntime(runtimePtr, handle)
	}

	@ReactMethod
	fun handleMessage(bytes: ReadableArray) {
		val data = ByteArray(bytes.size())
		for (i in 0 until bytes.size()) {
			data[i] = (bytes.getInt(i) and 0xff).toByte()
		}
		NipworkerRuntime.handleMessage(data)
	}

	@ReactMethod
	fun wake() {
		NipworkerRuntime.wake()
	}

	@ReactMethod
	fun setPrivateKey(secret: String) {
		NipworkerRuntime.setPrivateKey(secret)
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
	fun deinitEngine() {
		removeRuntimeListener?.invoke()
		removeRuntimeListener = null
	}

	private fun emitRuntimeData(data: ByteArray) {
		if (nativeIsByteRuntimeInstalled() && nativeQueueData(data)) {
			val payload = Arguments.createMap().apply {
				putInt("v", 1)
				putString("encoding", "queued")
			}
			emitData(payload)
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
		emitData(payload)
	}

	private fun emitData(payload: com.facebook.react.bridge.WritableMap) {
		reactContext
			.getJSModule(DeviceEventManagerModule.RCTDeviceEventEmitter::class.java)
			.emit(EVENT_NAME, payload)
	}
}
