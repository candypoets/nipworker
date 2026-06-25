import XCTest
@testable import NipworkerSwift

final class NostrManagerTests: XCTestCase {
    func testCreateShortId() async {
        let manager = NostrManager()
        let short = await manager.createShortId("hello")
        XCTAssertEqual(short, "hello")

        let long = await manager.createShortId(String(repeating: "a", count: 100))
        XCTAssertLessThanOrEqual(long.count, 63)
    }

    func testSubscribeCreatesBuffer() async {
        let manager = NostrManager()
        let buffer = await manager.subscribe(
            subscriptionId: "test-sub",
            requests: [RequestObject(kinds: [1], relays: ["wss://relay.damus.io"])]
        )
        XCTAssertEqual(ArrayBufferReader.getCurrentWritePosition(buffer: buffer), 4)
    }

    func testInitAndDeinit() {
        var manager: NostrManager? = NostrManager()
        manager = nil
        // Should not crash or leak
    }
}
