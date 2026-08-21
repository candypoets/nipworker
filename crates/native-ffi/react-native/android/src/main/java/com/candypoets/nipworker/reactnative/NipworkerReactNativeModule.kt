package com.candypoets.nipworker.reactnative

import android.content.Context
import com.facebook.react.bridge.ReadableArray
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.bridge.ReactMethod
import com.facebook.react.module.annotations.ReactModule
import com.facebook.react.turbomodule.core.interfaces.CallInvokerHolder

object NipworkerRuntime {
	private var sharedHandle: Long = 0L
	private var meshTransport: MeshBluetoothTransport? = null

	val handle: Long
		get() = synchronized(this) { sharedHandle }

	fun init(
		context: Context,
		defaultRelays: ReadableArray? = null,
		indexerRelays: ReadableArray? = null,
		meshBLEEnabled: Boolean = false,
		logLevel: String = "warn"
	): Long = synchronized(this) {
		NipworkerReactNativeModule.nipworkerSetLogLevel(logLevel)
		if (sharedHandle == 0L) {
			val cacheDir = context.filesDir.resolve("nipworker")
			sharedHandle = NipworkerReactNativeModule.nativeConfigureEngine(
				cacheDir.absolutePath,
				readableArrayToCsv(defaultRelays),
				readableArrayToCsv(indexerRelays),
				meshBLEEnabled
			)
		}
		if (meshBLEEnabled && sharedHandle != 0L) {
			val transport = meshTransport
				?: MeshBluetoothTransport(context.applicationContext, sharedHandle)
			meshTransport = transport
			transport.start()
		}
		sharedHandle
	}

	fun startMesh(context: Context): Boolean = synchronized(this) {
		if (sharedHandle == 0L) return@synchronized false
		val transport = meshTransport
			?: MeshBluetoothTransport(context.applicationContext, sharedHandle)
		meshTransport = transport
		transport.start()
	}

	fun stopMesh() = synchronized(this) {
		meshTransport?.stop()
		meshTransport = null
	}

	fun setMeshProfile(profileJson: String): Boolean = synchronized(this) {
		sharedHandle != 0L &&
			NipworkerReactNativeModule.nativeMeshSetProfile(sharedHandle, profileJson)
	}

	fun clearMeshProfile(): Boolean = synchronized(this) {
		sharedHandle != 0L && NipworkerReactNativeModule.nativeMeshClearProfile(sharedHandle)
	}

	fun handleMessage(bytes: ByteArray) {
		val handle = this.handle
		if (handle != 0L) NipworkerReactNativeModule.nipworkerHandleMessage(handle, bytes)
	}

	fun setPrivateKey(secret: String) {
		val handle = this.handle
		if (handle != 0L) NipworkerReactNativeModule.nipworkerSetPrivateKey(handle, secret)
	}

	fun clearSigner() {
		val handle = this.handle
		if (handle != 0L) NipworkerReactNativeModule.nipworkerClearSigner(handle)
	}

	fun removeSigner() {
		val handle = this.handle
		if (handle != 0L) NipworkerReactNativeModule.nipworkerRemoveSigner(handle)
	}

	fun wake() {
		val handle = this.handle
		if (handle != 0L) NipworkerReactNativeModule.nipworkerWake(handle)
	}

	fun deinit() = synchronized(this) {
		stopMesh()
		if (sharedHandle != 0L) {
			NipworkerReactNativeModule.nativeDeinitEngine()
			sharedHandle = 0L
		}
	}

	private fun readableArrayToCsv(values: ReadableArray?): String {
		if (values == null) return ""
		val relays = mutableListOf<String>()
		for (index in 0 until values.size()) {
			val relay = values.getString(index)?.trim()
			if (!relay.isNullOrEmpty() && !relay.contains(',')) relays.add(relay)
		}
		return relays.joinToString(",")
	}
}

@ReactModule(name = NipworkerReactNativeModule.NAME)
class NipworkerReactNativeModule(
	private val reactContext: ReactApplicationContext
) : NativeNipworkerReactNativeSpec(reactContext) {
	companion object {
		const val NAME = "NipworkerReactNativeModule"
		private const val STORAGE_NAME = "nipworker_storage"
		private const val MESH_PROFILE_STORAGE_KEY = "nipworker_mesh_profile"

		init {
			System.loadLibrary("nipworker_native_ffi")
			System.loadLibrary("nipworker_react_native")
		}

		@JvmStatic external fun nativeInstallTransport(
			runtimePtr: Long,
			callInvokerHolder: CallInvokerHolder
		): Long
		@JvmStatic external fun nativeInvalidateTransport(token: Long)
		@JvmStatic external fun nativeConfigureEngine(
			storagePath: String,
			defaultRelays: String,
			indexerRelays: String,
			meshBLEEnabled: Boolean
		): Long
		@JvmStatic external fun nativeDeinitEngine()

		@JvmStatic external fun nipworkerSetLogLevel(logLevel: String)
		@JvmStatic external fun nipworkerHandleMessage(handle: Long, bytes: ByteArray)
		@JvmStatic external fun nipworkerSetPrivateKey(handle: Long, secret: String)
		@JvmStatic external fun nipworkerClearSigner(handle: Long)
		@JvmStatic external fun nipworkerRemoveSigner(handle: Long)
		@JvmStatic external fun nipworkerWake(handle: Long)

		@JvmStatic external fun nativeMeshPeerConnected(handle: Long, peer: String, mtu: Int): Boolean
		@JvmStatic external fun nativeMeshPeerDisconnected(handle: Long, peer: String)
		@JvmStatic external fun nativeMeshReceiveFragment(
			handle: Long,
			peer: String,
			fragment: ByteArray
		): Boolean
		@JvmStatic external fun nativeMeshPopOutbound(handle: Long, peer: String): ByteArray?
		@JvmStatic external fun nativeMeshSetProfile(handle: Long, profileJson: String): Boolean
		@JvmStatic external fun nativeMeshClearProfile(handle: Long): Boolean
	}

	private var transportToken = 0L
	private val storage by lazy {
		reactContext.getSharedPreferences(STORAGE_NAME, Context.MODE_PRIVATE)
	}

	override fun getName(): String = NAME

	private fun ensureTransport(): Boolean {
		if (transportToken != 0L) return true
		val runtimePtr = reactContext.javaScriptContextHolder?.get() ?: 0L
		if (runtimePtr == 0L || !reactContext.hasActiveReactInstance()) return false
		val callInvokerHolder = reactContext.jsCallInvokerHolder ?: return false
		transportToken = nativeInstallTransport(runtimePtr, callInvokerHolder)
		return transportToken != 0L
	}

	@ReactMethod
	override fun initEngine(
		defaultRelays: ReadableArray,
		indexerRelays: ReadableArray,
		meshBLEEnabled: Boolean,
		logLevel: String
	) {
		// Bind the runtime-scoped CallInvoker transport before Rust can emit.
		check(ensureTransport()) { "Nipworker React Native JSI transport is unavailable" }
		NipworkerRuntime.init(
			reactContext,
			defaultRelays,
			indexerRelays,
			meshBLEEnabled,
			logLevel
		)
		storage.getString(MESH_PROFILE_STORAGE_KEY, null)?.let(NipworkerRuntime::setMeshProfile)
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun installByteRuntime(): Boolean = ensureTransport()

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun startMesh(): Boolean = NipworkerRuntime.startMesh(reactContext)

	@ReactMethod
	override fun stopMesh() = NipworkerRuntime.stopMesh()

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun setMeshProfile(profileJson: String): Boolean {
		if (!NipworkerRuntime.setMeshProfile(profileJson)) return false
		return storage.edit().putString(MESH_PROFILE_STORAGE_KEY, profileJson).commit()
	}

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun clearMeshProfile(): Boolean {
		val cleared = storage.edit().remove(MESH_PROFILE_STORAGE_KEY).commit()
		return NipworkerRuntime.clearMeshProfile() && cleared
	}

	@ReactMethod
	override fun handleMessage(bytes: ReadableArray) {
		val data = ByteArray(bytes.size())
		for (index in 0 until bytes.size()) data[index] = (bytes.getInt(index) and 0xff).toByte()
		NipworkerRuntime.handleMessage(data)
	}

	@ReactMethod override fun wake() = NipworkerRuntime.wake()
	@ReactMethod override fun setPrivateKey(secret: String) = NipworkerRuntime.setPrivateKey(secret)
	@ReactMethod override fun clearSigner() = NipworkerRuntime.clearSigner()
	@ReactMethod override fun removeSigner() = NipworkerRuntime.removeSigner()

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun getStorageItem(key: String): String? = storage.getString(key, null)

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun setStorageItem(key: String, value: String): Boolean =
		storage.edit().putString(key, value).commit()

	@ReactMethod(isBlockingSynchronousMethod = true)
	override fun removeStorageItem(key: String): Boolean = storage.edit().remove(key).commit()

	@ReactMethod
	override fun deinitEngine() {
		invalidateTransport()
		NipworkerRuntime.deinit()
	}

	override fun invalidate() {
		invalidateTransport()
		NipworkerRuntime.deinit()
		super.invalidate()
	}

	private fun invalidateTransport() {
		val token = transportToken
		transportToken = 0L
		if (token != 0L) nativeInvalidateTransport(token)
	}
}
