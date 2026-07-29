package com.candypoets.nipworker.reactnative

import java.nio.ByteBuffer
import java.nio.ByteOrder
import nostr.fb.MainMessage
import nostr.fb.Subscribe
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CacheFirstDefaultsTest {
	@Test
	fun requestAndSubscriptionCacheFirstDefaultsRemainDistinct() {
		val bytes = buildSubscribeMessage(
			"cache-first-defaults",
			listOf(
				NipworkerRequest(kinds = listOf(1)),
				NipworkerRequest(kinds = listOf(1), cacheFirst = true),
			),
			NipworkerSubscriptionOptions(),
		)

		val main = MainMessage.getRootAsMainMessage(
			ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN),
		)
		val subscribe = main.content(Subscribe()) as Subscribe

		assertFalse(subscribe.requests(0).cacheFirst())
		assertTrue(subscribe.requests(1).cacheFirst())
		assertTrue(subscribe.config().cacheFirst())
	}
}
