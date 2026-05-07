import XCTest
@testable import NipworkerSwift

final class IntegrationTests: XCTestCase {
    /// Smoke test: connect to a live relay and verify events flow through.
    /// This test requires internet access and may be flaky.
    func testUseSubscriptionFetchesRealEvents() async throws {
        let manager = NostrManager()
        let expectation = XCTestExpectation(description: "Receive events from relay")
        expectation.assertForOverFulfill = false

        var receivedEvents: [NostrEvent] = []

        let unsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "integration-test-\(UUID().uuidString)",
            requests: [RequestObject(kinds: [1], limit: 5, relays: ["wss://relay.damus.io"])],
            callback: { events in
                receivedEvents.append(contentsOf: events)
                if !receivedEvents.isEmpty {
                    expectation.fulfill()
                }
            }
        )

        // Give the subscription a moment to reach the Rust engine and open the WS.
        try await Task.sleep(nanoseconds: 500_000_000)

        await fulfillment(of: [expectation], timeout: 15)

        XCTAssertGreaterThan(receivedEvents.count, 0, "Should have received at least one event")
        XCTAssertFalse(receivedEvents[0].id.isEmpty, "Event should have an id")
        XCTAssertFalse(receivedEvents[0].pubkey.isEmpty, "Event should have a pubkey")

        unsubscribe()
    }

    func testUseRelayStatusSmokeTest() async throws {
        let manager = NostrManager()

        // useRelayStatus should return a valid stop closure even if no
        // status updates arrive (relay-status notifications depend on
        // Rust-side wiring that may not be enabled in this build).
        let stop = useRelayStatus(manager: manager) { _, _ in }

        // Give it a moment to set up.
        try await Task.sleep(nanoseconds: 100_000_000)

        // stop() should not crash.
        stop()
    }
}
