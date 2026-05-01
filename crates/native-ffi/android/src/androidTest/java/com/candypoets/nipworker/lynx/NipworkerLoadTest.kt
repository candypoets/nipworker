package com.candypoets.nipworker.lynx

import org.junit.Test

class NipworkerLoadTest {
	@Test
	fun loadNativeLibrary() {
		System.loadLibrary("nipworker_native_ffi")
	}
}
