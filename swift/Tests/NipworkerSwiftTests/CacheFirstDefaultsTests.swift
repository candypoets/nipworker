import FlatBuffers
import XCTest
@testable import NipworkerSwift

final class CacheFirstDefaultsTests: XCTestCase {
    func testCachePolicyIsSerializedPerRequest() {
        let data = buildSubscribeMessage(
            subId: "cache-first-defaults",
            requests: [
                RequestObject(kinds: [1]),
                RequestObject(kinds: [1], cacheFirst: true),
            ],
            options: SubscriptionConfig()
        )

        let buffer = ByteBuffer(data: data)
        let rootOffset = buffer.read(def: Int32.self, position: 0)
        let main = nostr_fb_MainMessage(buffer, o: rootOffset)
        let subscribe: nostr_fb_Subscribe = main.content(type: nostr_fb_Subscribe.self)

        XCTAssertFalse(subscribe.requests[0].cacheFirst)
        XCTAssertTrue(subscribe.requests[1].cacheFirst)
        // Retained only for wire compatibility; no longer part of SubscriptionConfig.
        XCTAssertTrue(subscribe.config.cacheFirst)
        XCTAssertFalse(subscribe.config.skipCache)
    }
}
