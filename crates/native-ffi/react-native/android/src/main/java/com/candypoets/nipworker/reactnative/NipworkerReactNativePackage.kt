package com.candypoets.nipworker.reactnative

import com.facebook.react.BaseReactPackage
import com.facebook.react.bridge.NativeModule
import com.facebook.react.bridge.ReactApplicationContext
import com.facebook.react.module.model.ReactModuleInfo
import com.facebook.react.module.model.ReactModuleInfoProvider
import com.facebook.react.uimanager.ViewManager

class NipworkerReactNativePackage : BaseReactPackage() {
	override fun getModule(name: String, reactContext: ReactApplicationContext): NativeModule? {
		return when (name) {
			NipworkerReactNativeModule.NAME -> NipworkerReactNativeModule(reactContext)
			else -> null
		}
	}

	override fun getReactModuleInfoProvider(): ReactModuleInfoProvider {
		val moduleInfo = ReactModuleInfo(
			NipworkerReactNativeModule.NAME,
			NipworkerReactNativeModule::class.java.name,
			false,
			false,
			false,
			true
		)
		return ReactModuleInfoProvider {
			mapOf(NipworkerReactNativeModule.NAME to moduleInfo)
		}
	}

	override fun createViewManagers(
		reactContext: ReactApplicationContext
	): List<ViewManager<*, *>> {
		return emptyList()
	}
}
