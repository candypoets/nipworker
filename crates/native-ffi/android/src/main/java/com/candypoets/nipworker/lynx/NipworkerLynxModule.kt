package com.candypoets.nipworker.lynx

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Base64
import android.util.Log
import com.lynx.jsbridge.LynxMethod
import com.lynx.jsbridge.LynxModule
import com.lynx.react.bridge.JavaOnlyArray
import com.lynx.react.bridge.JavaOnlyMap
import java.util.concurrent.atomic.AtomicLong

/**
 * Lynx native module for NIPWorker.
 *
 * Host apps should register this class under [MODULE_NAME], for example:
 * "NipworkerLynxModule" to SparklingLynxModuleWrapper(NipworkerLynxModule::class.java, null)
 */
class NipworkerLynxModule(context: Context) : LynxModule(context) {
	companion object {
		const val MODULE_NAME = "NipworkerLynxModule"
		private const val EVENT_NAME = "NipworkerEvent"
		private const val TAG = "Nipworker"

		init {
			System.loadLibrary("nipworker_native_ffi")
		}

		private val nextUserdata = AtomicLong(1L)
		private val mainHandler = Handler(Looper.getMainLooper())

		@Volatile
		var eventSender: ((String, JavaOnlyArray) -> Unit)? = null

		@Volatile
		private var sharedHandle: Long = 0L

		@Volatile
		private var sharedUserdata: Long = 0L

		@JvmStatic
		fun onNativeData(userdata: Long, data: ByteArray) {
			val base64 = Base64.encodeToString(data, Base64.NO_WRAP)
			val payload = JavaOnlyMap().apply {
				putInt("v", 1)
				putString("encoding", "base64")
				putString("data", base64)
			}
			val params = JavaOnlyArray().apply {
				pushMap(payload)
			}

			val sender = eventSender
			if (sender != null) {
				mainHandler.post {
					sender.invoke(EVENT_NAME, params)
				}
			} else {
				Log.w(TAG, "eventSender unavailable for userdata=$userdata, event dropped")
			}
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

	@LynxMethod
	fun init() {
		if (sharedHandle == 0L) {
			sharedUserdata = nextUserdata.getAndIncrement()
			sharedHandle = nipworkerInit(sharedUserdata)
		}
	}

	@LynxMethod
	fun handleMessage(bytes: ByteArray) {
		if (sharedHandle != 0L) {
			nipworkerHandleMessage(sharedHandle, bytes)
		}
	}

	@LynxMethod
	fun setPrivateKey(secret: String) {
		if (sharedHandle != 0L) {
			nipworkerSetPrivateKey(sharedHandle, secret)
		}
	}

	@LynxMethod
	fun deinit() {
		if (sharedHandle != 0L) {
			nipworkerDeinit(sharedHandle)
			sharedHandle = 0L
			sharedUserdata = 0L
		}
	}
}
