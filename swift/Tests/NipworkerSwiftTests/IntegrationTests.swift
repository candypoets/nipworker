import XCTest
@testable import NipworkerSwift

final class IntegrationTests: XCTestCase {
    /// Smoke test: connect to a live relay and verify events flow through.
    /// This test requires internet access and may be flaky.
    func testUseSubscriptionFetchesRealEvents() async throws {
        let manager = NostrManager()
        let expectation = XCTestExpectation(description: "Receive events from relay")
        expectation.assertForOverFulfill = false

        var receivedMessages: [WorkerMessageView] = []

        let unsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "integration-test-\(UUID().uuidString)",
            requests: [RequestObject(kinds: [1], limit: 5, relays: ["wss://relay.damus.io"])],
            callback: { messages in
                receivedMessages.append(contentsOf: messages)
                if receivedMessages.contains(where: { $0.kind1 != nil }) {
                    expectation.fulfill()
                }
            }
        )

        // Give the subscription a moment to reach the Rust engine and open the WS.
        try await Task.sleep(nanoseconds: 500_000_000)

        await fulfillment(of: [expectation], timeout: 15)

        let parsedMessages = receivedMessages.compactMap { $0.parsedEvent }
        let kind1Messages = receivedMessages.compactMap { $0.kind1 }
        XCTAssertGreaterThan(parsedMessages.count, 0, "Should have received at least one parsed event")
        XCTAssertGreaterThan(kind1Messages.count, 0, "Should have received at least one kind-1 parsed event")
        XCTAssertTrue(receivedMessages.contains(where: { $0.contentType == .parsedevent }))
        XCTAssertTrue(receivedMessages.contains(where: { $0.kind0 == nil }))
        XCTAssertFalse(parsedMessages[0].id.isEmpty, "Parsed event should have an id")
        XCTAssertFalse(parsedMessages[0].pubkey.isEmpty, "Parsed event should have a pubkey")

        unsubscribe()
    }

    func testUseSubscriptionCacheFirstReturnsAlreadyFetchedNote() async throws {
        let manager = NostrManager()
        let firstExpectation = XCTestExpectation(description: "Receive live event to populate cache")
        firstExpectation.assertForOverFulfill = false

        var fetchedId: String?
        let firstUnsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "cache-first-live-\(UUID().uuidString)",
            requests: [RequestObject(kinds: [1], limit: 1, relays: ["wss://relay.damus.io"])],
            callback: { messages in
                if let parsed = messages.compactMap({ $0.parsedEvent }).first {
                    fetchedId = parsed.id
                    firstExpectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        try await Task.sleep(nanoseconds: 500_000_000)
        await fulfillment(of: [firstExpectation], timeout: 15)
        firstUnsubscribe()

        let id = try XCTUnwrap(fetchedId)
        let secondExpectation = XCTestExpectation(description: "Receive same event from cache-first lookup")
        secondExpectation.assertForOverFulfill = false

        var cachedIds: [String] = []
        let secondUnsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "cache-first-cached-\(UUID().uuidString)",
            requests: [
                RequestObject(
                    ids: [id],
                    kinds: [1],
                    limit: 1,
                    relays: ["wss://127.0.0.1:9"],
                    cacheFirst: true
                )
            ],
            callback: { messages in
                let ids = messages.compactMap { $0.parsedEvent?.id }
                cachedIds.append(contentsOf: ids)
                if ids.contains(id) {
                    secondExpectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true, timeoutMs: 500)
        )

        await fulfillment(of: [secondExpectation], timeout: 5)
        XCTAssertTrue(cachedIds.contains(id), "Second subscription should receive the already fetched note from cache")
        secondUnsubscribe()
    }

    func testCounterPipelineDoesNotCountUnrelatedCachedKind1EventsForETagRequest() async throws {
        let manager = NostrManager()
        let targetEventId = "870db1dc9eb056c0791882762065381f0814cc1ba1f89910a465bc8b1f205c9a"
        let populateExpectation = XCTestExpectation(description: "Populate cache with general kind-1 events")
        populateExpectation.assertForOverFulfill = false

        let state = LockedCounterRegressionState()
        let populateUnsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "counter-cache-populate-\(UUID().uuidString)",
            requests: [RequestObject(kinds: [1], limit: 20, relays: ["wss://relay.damus.io"])],
            callback: { messages in
                let populatedCount = state.addPopulatedEvents(messages.compactMap(\.parsedEvent))
                if populatedCount >= 5 {
                    populateExpectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        try await Task.sleep(nanoseconds: 500_000_000)
        await fulfillment(of: [populateExpectation], timeout: 15)
        populateUnsubscribe()

        XCTAssertFalse(state.containsPopulatedId(targetEventId), "The seed query should not only fetch the target event")

        let countExpectation = XCTestExpectation(description: "Receive counter response from cache")
        countExpectation.assertForOverFulfill = false

        let countUnsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "counter-cache-etag-\(UUID().uuidString)",
            requests: [
                RequestObject(
                    kinds: [1],
                    tags: ["#e": [targetEventId]],
                    limit: 500,
                    relays: ["wss://127.0.0.1:9"],
                    cacheFirst: true
                ),
                RequestObject(
                    kinds: [6, 7, 17],
                    tags: ["#e": [targetEventId]],
                    limit: 500,
                    relays: ["wss://127.0.0.1:9"],
                    cacheFirst: true
                )
            ],
            callback: { messages in
                let countResponseCount = state.addCountMessages(messages)
                if countResponseCount >= 4 {
                    countExpectation.fulfill()
                }
            },
            options: SubscriptionConfig(
                pipeline: [PipeConfig(.counter(kinds: [1, 6, 7, 17], pubkey: ""))],
                closeOnEose: true,
                cacheFirst: true,
                timeoutMs: 500,
                cacheOnly: true
            )
        )

        await fulfillment(of: [countExpectation], timeout: 5)
        countUnsubscribe()

        let countsByKind = state.countsByKind()
        let eventIds = state.unexpectedEventIds()
        XCTAssertTrue(eventIds.isEmpty, "Counter pipeline should emit count responses, not cached events")
        XCTAssertEqual(countsByKind[1], 0, "Kind-1 count must honor the #e filter instead of counting the whole cached kind-1 set")
        XCTAssertEqual(countsByKind[6], 0)
        XCTAssertEqual(countsByKind[7], 0)
        XCTAssertEqual(countsByKind[17], 0)
    }

    func testCounterPipelineCountsCachedKind7LikesForETagRequest() async throws {
        let manager = NostrManager()
        let seedState = LockedKind7SeedState()
        let seedExpectation = XCTestExpectation(description: "Populate cache with a kind-7 like that has an e tag")
        seedExpectation.assertForOverFulfill = false

        let seedUnsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "counter-kind7-seed-\(UUID().uuidString)",
            requests: [RequestObject(kinds: [7], limit: 50, relays: ["wss://relay.damus.io"])],
            callback: { messages in
                if seedState.record(messages: messages) != nil {
                    seedExpectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        try await Task.sleep(nanoseconds: 500_000_000)
        await fulfillment(of: [seedExpectation], timeout: 15)
        seedUnsubscribe()

        let targetEventId = try XCTUnwrap(seedState.targetEventId())
        let countState = LockedCounterRegressionState()
        let countExpectation = XCTestExpectation(description: "Receive kind-7 counter response from cache")
        countExpectation.assertForOverFulfill = false

        let countUnsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "counter-kind7-etag-\(UUID().uuidString)",
            requests: [
                RequestObject(
                    kinds: [1],
                    tags: ["#e": [targetEventId]],
                    limit: 500,
                    relays: ["wss://127.0.0.1:9"],
                    cacheFirst: true
                ),
                RequestObject(
                    kinds: [6, 7, 17],
                    tags: ["#e": [targetEventId]],
                    limit: 500,
                    relays: ["wss://127.0.0.1:9"],
                    cacheFirst: true
                )
            ],
            callback: { messages in
                if countState.addCountMessages(messages) >= 4 {
                    countExpectation.fulfill()
                }
            },
            options: SubscriptionConfig(
                pipeline: [PipeConfig(.counter(kinds: [1, 6, 7, 17], pubkey: ""))],
                closeOnEose: true,
                cacheFirst: true,
                timeoutMs: 500,
                cacheOnly: true
            )
        )

        await fulfillment(of: [countExpectation], timeout: 5)
        countUnsubscribe()

        let countsByKind = countState.countsByKind()
        XCTAssertGreaterThanOrEqual(countsByKind[7] ?? 0, 1, "Kind-7 count should include the cached like for the target #e")
    }

    func testUseSubscriptionResolvesKnownNoteByIdAcrossRelays() async throws {
        let fixture = try ReplayFixture(log: Self.knownNoteReplayLog)
        let manager = NostrManager()
        let eventId = fixture.noteId
        let relays = fixture.requests.first?.relays ?? []

        let first = try await resolveKnownNote(manager: manager, eventId: eventId, relays: relays, label: "first")
        XCTAssertEqual(first.parsedEvent?.id, eventId)
        XCTAssertEqual(first.parsedEvent?.kind, 1)
        XCTAssertNotNil(first.kind1)

        let second = try await resolveKnownNote(manager: manager, eventId: eventId, relays: relays, label: "second")
        XCTAssertEqual(second.parsedEvent?.id, eventId)
        XCTAssertEqual(second.parsedEvent?.kind, 1)
        XCTAssertNotNil(second.kind1)
    }

    func testReplayLogFixtureResolvesKnownNote() async throws {
        let fixture = try ReplayFixture(log: Self.knownNoteReplayLog)
        let manager = NostrManager()
        let expectation = XCTestExpectation(description: "Replay log resolves known note")
        expectation.assertForOverFulfill = false

        var resolvedId: String?
        var resolvedKind: UInt16?
        var resolvedHasKind1 = false
        let unsubscribe = useSubscription(
            manager: manager,
            subscriptionId: fixture.subscriptionId,
            requests: fixture.requests,
            callback: { messages in
                if let found = messages.first(where: { $0.parsedEvent?.id == fixture.noteId }) {
                    resolvedId = found.parsedEvent?.id
                    resolvedKind = found.parsedEvent?.kind
                    resolvedHasKind1 = found.kind1 != nil
                    expectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        try await Task.sleep(nanoseconds: 500_000_000)
        await fulfillment(of: [expectation], timeout: 15)
        unsubscribe()

        XCTAssertEqual(resolvedId, fixture.noteId)
        XCTAssertEqual(resolvedKind, 1)
        XCTAssertTrue(resolvedHasKind1)
    }

    func testUnsubscribeWithoutCleanupKeepsBufferForSameSubscriptionId() async throws {
        let fixture = try ReplayFixture(log: Self.knownNoteReplayLog)
        let manager = NostrManager()
        let canonicalId = manager.createShortId(fixture.subscriptionId)

        let firstBuffer = await manager.subscribe(
            subscriptionId: fixture.subscriptionId,
            requests: fixture.requests,
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        let firstMessages = try await waitForMessages(
            manager: manager,
            subscriptionId: canonicalId,
            matching: { messages in
                messages.contains { $0.parsedEvent?.id == fixture.noteId }
            }
        )
        XCTAssertTrue(firstMessages.contains { $0.parsedEvent?.id == fixture.noteId })

        await manager.unsubscribe(subscriptionId: fixture.subscriptionId)

        let secondBuffer = await manager.subscribe(
            subscriptionId: fixture.subscriptionId,
            requests: fixture.requests,
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        XCTAssertTrue(firstBuffer === secondBuffer, "Same sub id should reuse the existing SubscriptionBuffer before cleanup")

        let reread = await manager.readWorkerMessages(for: canonicalId, from: 4)
        XCTAssertTrue(
            reread.messages.contains { $0.parsedEvent?.id == fixture.noteId },
            "Reused buffer should still contain already written messages when cleanup has not run"
        )
    }

    func testUseSubscriptionCanReuseSameSubscriptionIdAfterUnsubscribeWithoutCleanup() async throws {
        let fixture = try ReplayFixture(log: Self.knownNoteReplayLog)
        let manager = NostrManager()

        let first = try await subscribeWithHook(
            manager: manager,
            subscriptionId: fixture.subscriptionId,
            requests: fixture.requests,
            noteId: fixture.noteId,
            label: "first"
        )
        XCTAssertEqual(first.id, fixture.noteId)
        XCTAssertEqual(first.kind, 1)
        XCTAssertGreaterThan(first.totalCallbackMessageCount, 0)

        let second = try await subscribeWithHook(
            manager: manager,
            subscriptionId: fixture.subscriptionId,
            requests: fixture.requests,
            noteId: fixture.noteId,
            label: "second"
        )
        XCTAssertEqual(second.id, fixture.noteId)
        XCTAssertEqual(second.kind, 1)
        XCTAssertGreaterThan(second.totalCallbackMessageCount, 0)
    }

    func testUseSubscriptionRunsNutsIOSReplayLogReusedSubscriptionIds() async throws {
        let logURL = URL(fileURLWithPath: "/tmp/nuts-ios-nipworker-replay-20260507-220321.log")
        guard FileManager.default.fileExists(atPath: logURL.path) else {
            throw XCTSkip("Replay log is not present at \(logURL.path)")
        }

        let log = try String(contentsOf: logURL, encoding: .utf8)
        let replay = try ReplayLog(log: log)
        let repeatedFixtures = replay.repeatedSubscriptionFixtures
        XCTAssertEqual(repeatedFixtures.count, 7)
        let unavailableNoteIds = Set([
            "e6304b787fb1a05b65eb59973d23f682442650bb307301c250ce3a3f21b5d2cf",
            "c80ca8ff0bbef041943ea19d871cf1a39fab517f331c412dd40611c55e9e0e6f"
        ])
        let resolvableFixtures = repeatedFixtures.filter { !unavailableNoteIds.contains($0.noteId) }
        XCTAssertEqual(resolvableFixtures.count, 5)

        let manager = NostrManager()
        for fixture in resolvableFixtures {
            let first = try await subscribeWithHook(
                manager: manager,
                subscriptionId: fixture.subscriptionId,
                requests: fixture.requests,
                noteId: fixture.noteId,
                label: "replay first \(fixture.noteId)"
            )
            XCTAssertEqual(first.id, fixture.noteId)

            let second = try await subscribeWithHook(
                manager: manager,
                subscriptionId: fixture.subscriptionId,
                requests: fixture.requests,
                noteId: fixture.noteId,
                label: "replay second \(fixture.noteId)"
            )
            XCTAssertEqual(second.id, fixture.noteId)
        }
    }

    private func subscribeWithHook(
        manager: NostrManager,
        subscriptionId: String,
        requests: [RequestObject],
        noteId: String,
        label: String
    ) async throws -> (id: String, kind: UInt16, totalCallbackMessageCount: Int) {
        let expectation = XCTestExpectation(description: "useSubscription resolves \(label)")
        expectation.assertForOverFulfill = false
        var resolvedId: String?
        var resolvedKind: UInt16?
        var callbackMessages: [WorkerMessageView] = []

        let unsubscribe = useSubscription(
            manager: manager,
            subscriptionId: subscriptionId,
            requests: requests,
            callback: { messages in
                callbackMessages.append(contentsOf: messages)
                if let found = callbackMessages.first(where: { $0.parsedEvent?.id == noteId }) {
                    resolvedId = found.parsedEvent?.id
                    resolvedKind = found.parsedEvent?.kind
                    expectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        await fulfillment(of: [expectation], timeout: 15)
        unsubscribe()

        return (
            id: try XCTUnwrap(resolvedId),
            kind: try XCTUnwrap(resolvedKind),
            totalCallbackMessageCount: callbackMessages.count
        )
    }

    private func waitForMessages(
        manager: NostrManager,
        subscriptionId: String,
        matching predicate: ([WorkerMessageView]) -> Bool
    ) async throws -> [WorkerMessageView] {
        var readPosition = 4
        var allMessages: [WorkerMessageView] = []
        let deadline = Date().addingTimeInterval(15)

        while Date() < deadline {
            let result = await manager.readWorkerMessages(for: subscriptionId, from: readPosition)
            readPosition = result.newPosition
            if !result.messages.isEmpty {
                allMessages.append(contentsOf: result.messages)
                if predicate(allMessages) {
                    return allMessages
                }
            }
            try await Task.sleep(nanoseconds: 100_000_000)
        }

        XCTFail("Timed out waiting for subscription messages")
        return allMessages
    }

    private func resolveKnownNote(
        manager: NostrManager,
        eventId: String,
        relays: [String],
        label: String
    ) async throws -> WorkerMessageView {
        let expectation = XCTestExpectation(description: "Resolve known note by id \(label)")
        expectation.assertForOverFulfill = false

        var resolved: WorkerMessageView?
        let unsubscribe = useSubscription(
            manager: manager,
            subscriptionId: "resolve-known-note-\(label)-\(UUID().uuidString)",
            requests: [
                RequestObject(
                    ids: [eventId],
                    relays: relays,
                    cacheFirst: true
                )
            ],
            callback: { messages in
                if let found = messages.first(where: { $0.parsedEvent?.id == eventId }) {
                    resolved = found
                    expectation.fulfill()
                }
            },
            options: SubscriptionConfig(closeOnEose: true, cacheFirst: true)
        )

        try await Task.sleep(nanoseconds: 500_000_000)
        await fulfillment(of: [expectation], timeout: 15)
        unsubscribe()
        return try XCTUnwrap(resolved)
    }

    private static let knownNoteReplayLog = """
    NIPWORKER_REPLAY_SUB metadata {"noteId":"75f98e4486d41dcf5cf645e42f83b02d04b4c7f4a9ad24dc03f6f0a1a6ef5cb5","relays":["wss://relay.damus.io","wss://nos.lol","wss://relay.primal.net","wss://nos.lol/cipher-zulu"],"requestCount":1,"subscriptionId":"note_75f98e4486d41dcf5cf645e42f83b02d04b4c7f4a9ad24dc03f6f0a1a6ef5cb5_0","targetKey":"75f98e4486d41dcf5cf645e42f83b02d04b4c7f4a9ad24dc03f6f0a1a6ef5cb5"}
    NIPWORKER_REPLAY_SUB request[0] {"cacheFirst":true,"ids":["75f98e4486d41dcf5cf645e42f83b02d04b4c7f4a9ad24dc03f6f0a1a6ef5cb5"],"limit":5,"relays":["wss://relay.damus.io","wss://nos.lol","wss://relay.primal.net","wss://nos.lol/cipher-zulu"]}
    NIPWORKER_REPLAY_SUB wire[0] ["REQ","note_75f98e4486d41dcf5cf645e42f83b02d04b4c7f4a9ad24dc03f6f0a1a6ef5cb5_0",{"ids":["75f98e4486d41dcf5cf645e42f83b02d04b4c7f4a9ad24dc03f6f0a1a6ef5cb5"],"limit":5}]
    """

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

private struct ReplayFixture {
    let subscriptionId: String
    let noteId: String
    let requests: [RequestObject]

    init(subscriptionId: String, noteId: String, requests: [RequestObject]) {
        self.subscriptionId = subscriptionId
        self.noteId = noteId
        self.requests = requests
    }

    init(log: String) throws {
        var subscriptionId: String?
        var noteId: String?
        var requests: [RequestObject] = []

        for line in log.split(separator: "\n").map(String.init) {
            if let json = line.replayJSONPayload(prefix: "NIPWORKER_REPLAY_SUB metadata "),
               let object = try JSONSerialization.jsonObject(with: json) as? [String: Any] {
                subscriptionId = object["subscriptionId"] as? String
                noteId = object["noteId"] as? String
            } else if let json = line.replayJSONPayload(matching: #"NIPWORKER_REPLAY_SUB request\[\d+\] "#),
                      let object = try JSONSerialization.jsonObject(with: json) as? [String: Any] {
                requests.append(Self.request(from: object))
            }
        }

        self.subscriptionId = try XCTUnwrap(subscriptionId)
        self.noteId = try XCTUnwrap(noteId)
        self.requests = requests
    }

    fileprivate static func request(from object: [String: Any]) -> RequestObject {
        var tags: [String: [String]] = [:]
        for (key, value) in object where key.hasPrefix("#") {
            if let values = value as? [String] {
                tags[key] = values
            }
        }

        return RequestObject(
            ids: object["ids"] as? [String],
            authors: object["authors"] as? [String],
            kinds: (object["kinds"] as? [Int])?.map(UInt16.init),
            tags: tags.isEmpty ? nil : tags,
            since: object["since"] as? Int,
            until: object["until"] as? Int,
            limit: object["limit"] as? Int,
            search: object["search"] as? String,
            relays: object["relays"] as? [String] ?? [],
            closeOnEOSE: object["closeOnEOSE"] as? Bool,
            cacheFirst: object["cacheFirst"] as? Bool,
            noCache: object["noCache"] as? Bool,
            maxRelays: object["maxRelays"] as? UInt16
        )
    }
}

private final class LockedCounterRegressionState {
    private let lock = NSLock()
    private var populatedIds = Set<String>()
    private var countResponses: [(kind: UInt16, count: UInt32)] = []
    private var unexpectedEvents: [String] = []

    func addPopulatedEvents(_ events: [nostr_fb_ParsedEvent]) -> Int {
        lock.lock()
        defer { lock.unlock() }

        for event in events where event.kind == 1 {
            populatedIds.insert(event.id)
        }
        return populatedIds.count
    }

    func containsPopulatedId(_ id: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return populatedIds.contains(id)
    }

    func addCountMessages(_ messages: [WorkerMessageView]) -> Int {
        lock.lock()
        defer { lock.unlock() }

        countResponses.append(contentsOf: messages.compactMap { message in
            message.countResponse.map { (kind: $0.kind, count: $0.count) }
        })
        unexpectedEvents.append(contentsOf: messages.compactMap { $0.parsedEvent?.id })
        return countResponses.count
    }

    func countsByKind() -> [UInt16: UInt32] {
        lock.lock()
        defer { lock.unlock() }
        return countResponses.reduce(into: [UInt16: UInt32]()) { result, response in
            result[response.kind] = response.count
        }
    }

    func unexpectedEventIds() -> [String] {
        lock.lock()
        defer { lock.unlock() }
        return unexpectedEvents
    }
}

private final class LockedKind7SeedState {
    private let lock = NSLock()
    private var target: String?

    func record(messages: [WorkerMessageView]) -> String? {
        lock.lock()
        defer { lock.unlock() }

        if let target {
            return target
        }

        for event in messages.compactMap(\.parsedEvent) where event.kind == 7 {
            for tag in event.tags {
                let items = tag.items.compactMap { $0 }
                if items.count >= 2, items[0] == "e" {
                    target = items[1]
                    return items[1]
                }
            }
        }
        return nil
    }

    func targetEventId() -> String? {
        lock.lock()
        defer { lock.unlock() }
        return target
    }
}

private struct ReplayLog {
    let fixtures: [ReplayFixture]

    var repeatedSubscriptionFixtures: [ReplayFixture] {
        let counts = fixtures.reduce(into: [String: Int]()) { result, fixture in
            result[fixture.subscriptionId, default: 0] += 1
        }

        var seen = Set<String>()
        return fixtures.filter { fixture in
            guard counts[fixture.subscriptionId, default: 0] > 1,
                  !seen.contains(fixture.subscriptionId) else {
                return false
            }
            seen.insert(fixture.subscriptionId)
            return true
        }
    }

    init(log: String) throws {
        var fixtures: [ReplayFixture] = []
        var subscriptionId: String?
        var noteId: String?
        var requests: [RequestObject] = []

        func appendCurrentFixture() throws {
            guard subscriptionId != nil || noteId != nil || !requests.isEmpty else { return }
            fixtures.append(ReplayFixture(
                subscriptionId: try XCTUnwrap(subscriptionId),
                noteId: try XCTUnwrap(noteId),
                requests: requests
            ))
        }

        for line in log.split(separator: "\n").map(String.init) {
            if let json = line.replayJSONPayload(prefix: "NIPWORKER_REPLAY_SUB metadata "),
               let object = try JSONSerialization.jsonObject(with: json) as? [String: Any] {
                try appendCurrentFixture()
                subscriptionId = object["subscriptionId"] as? String
                noteId = object["noteId"] as? String
                requests = []
            } else if let json = line.replayJSONPayload(matching: #"NIPWORKER_REPLAY_SUB request\[\d+\] "#),
                      let object = try JSONSerialization.jsonObject(with: json) as? [String: Any] {
                requests.append(ReplayFixture.request(from: object))
            } else if line.hasPrefix("NIPWORKER_REPLAY_UNSUB metadata ") {
                try appendCurrentFixture()
                subscriptionId = nil
                noteId = nil
                requests = []
            }
        }

        try appendCurrentFixture()
        self.fixtures = fixtures
    }
}

private extension String {
    func replayJSONPayload(prefix: String) -> Data? {
        guard hasPrefix(prefix) else { return nil }
        return String(dropFirst(prefix.count)).data(using: .utf8)
    }

    func replayJSONPayload(matching pattern: String) -> Data? {
        guard let regex = try? NSRegularExpression(pattern: pattern),
              let match = regex.firstMatch(in: self, range: NSRange(startIndex..., in: self)),
              match.range.location == 0,
              let range = Range(match.range, in: self) else {
            return nil
        }
        return String(self[range.upperBound...]).data(using: .utf8)
    }
}
