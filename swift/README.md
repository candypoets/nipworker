# NipworkerSwift

Swift bindings for NIPWorker — a high-performance Nostr client library powered by Rust WebAssembly and FlatBuffers. This package wraps the Rust native FFI layer to provide an async-first, actor-safe Swift API for subscribing to relays, publishing events, and managing cryptographic signers.

## Requirements

- **iOS 14+** / **macOS 13+**
- **Swift 5.9+**
- **Xcode 15+** (recommended)

## Installation

### Xcode — Local Package (recommended while iterating)

1. Open your iOS/macOS project in Xcode.
2. Go to **File → Add Package Dependencies…**
3. Click **Add Local…** and select the `swift/` folder inside this repository.
4. Add `NipworkerSwift` to your app target.

> **Note on the XCFramework path**: `Package.swift` references the XCFramework via a relative path (`../crates/native-ffi/ios/`). Some Xcode/SPM versions reject binary targets that live outside the package root. If you hit this, copy or symlink the framework into the package:
>
> ```bash
> mkdir -p swift/Frameworks
> cp -R crates/native-ffi/ios/NipworkerNativeFFI.xcframework swift/Frameworks/
> ```
>
> Then change the `binaryTarget` path in `swift/Package.swift` to `"Frameworks/NipworkerNativeFFI.xcframework"`.

### Swift Package Manager

Add the following to your `Package.swift`:

```swift
// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "YourApp",
    platforms: [.iOS(.v14), .macOS(.v13)],
    dependencies: [
        .package(name: "NipworkerSwift", path: "../path-to-nipworker-repo/swift")
    ],
    targets: [
        .executableTarget(
            name: "YourApp",
            dependencies: ["NipworkerSwift"]
        )
    ]
)
```

## Quick Start

```swift
import NipworkerSwift

let manager = NostrManager()

let request = RequestObject(
    filters: [.init(kinds: [1], limit: 10)],
    relays: ["wss://relay.damus.io"]
)

let buffer = await manager.subscribe(
    subscriptionId: "feed",
    requests: [request]
)

let unsub = useSubscription(
    manager: manager,
    subscriptionId: "feed",
    requests: [request]
) { events in
    for event in events {
        print("Event:", event.id, event.content)
    }
}
```

## API Reference

### `NostrManager`

The core actor that owns the Rust engine handle. All methods are `async` or return values synchronously from the actor's isolated state.

```swift
public actor NostrManager {
    public init(config: NostrManagerConfig = NostrManagerConfig())

    // Subscription
    public func subscribe(
        subscriptionId: String,
        requests: [RequestObject],
        options: SubscriptionConfig = SubscriptionConfig()
    ) -> SubscriptionBuffer

    public func unsubscribe(subscriptionId: String)

    // Publish
    public func publish(
        publishId: String,
        event: NostrEvent,
        defaultRelays: [String] = [],
        optimisticSubIds: [String] = []
    ) -> SubscriptionBuffer

    // Signer
    public func setSigner(_ signer: SignerTypeName)
    public func getPublicKey() async throws -> String
    public func signEvent(_ template: EventTemplate) async throws -> NostrEvent
    public func logout()

    // Read helpers
    public func readEvents(for subId: String, from position: Int) -> (events: [NostrEvent], newPosition: Int)
    public func readPublishStatuses(for publishId: String, from position: Int) -> (statuses: [String: PublishStatus], newPosition: Int)
    public func getRelayStatuses() -> [String: RelayStatus]
    public func getActivePubkey() -> String?

    public func cleanup()
}
```

### Models

#### `RequestObject`

Defines a Nostr filter + relay set.

```swift
public struct RequestObject {
    public var ids: [String]?
    public var authors: [String]?
    public var kinds: [UInt16]?
    public var tags: [String: [String]]?
    public var since: Int?
    public var until: Int?
    public var limit: Int?
    public var search: String?
    public var relays: [String]
    public var closeOnEOSE: Bool?
    public var cacheFirst: Bool?
    public var noCache: Bool?
    public var maxRelays: UInt16?
}
```

#### `SubscriptionConfig`

Controls subscription behavior.

```swift
public struct SubscriptionConfig {
    public var pipeline: [PipeConfig]?
    public var closeOnEose: Bool
    public var cacheFirst: Bool
    public var timeoutMs: UInt64?
    public var maxEvents: UInt32?
    public var skipCache: Bool
    public var force: Bool
    public var bytesPerEvent: UInt32
    public var isSlow: Bool
    public var pagination: String?
    public var cacheOnly: Bool
}
```

#### `NostrEvent`

```swift
public struct NostrEvent: Identifiable, Equatable, Sendable {
    public var id: String
    public var pubkey: String
    public var kind: UInt16
    public var content: String
    public var tags: [[String]]
    public var createdAt: Int
    public var sig: String
}
```

#### `EventTemplate`

Used when signing a new event.

```swift
public struct EventTemplate {
    public var kind: UInt16
    public var content: String
    public var tags: [[String]]
}
```

#### `SignerTypeName`

```swift
public enum SignerTypeName: Sendable {
    case pubkey(String)
    case privkey(String)
}
```

#### `RelayStatus`

```swift
public enum RelayStatus: Equatable, Sendable {
    case connected
    case failed
    case closed(String?)
}
```

#### `PublishStatus`

```swift
public enum PublishStatus: Equatable, Sendable {
    case pending
    case success(String)
    case failed(String)
}
```

### Hooks

Callback-based helpers that mirror the JS `useSubscription` / `usePublish` / `useRelayStatus` hooks. They return an unsubscribe function.

```swift
public func useSubscription(
    manager: NostrManager,
    subscriptionId: String,
    requests: [RequestObject],
    callback: @escaping ([NostrEvent]) -> Void,
    options: SubscriptionConfig = SubscriptionConfig()
) -> () -> Void

public func usePublish(
    manager: NostrManager,
    publishId: String,
    event: NostrEvent,
    callback: @escaping ([String: PublishStatus]) -> Void,
    defaultRelays: [String] = [],
    optimisticSubIds: [String] = []
) -> () -> Void

public func useRelayStatus(
    manager: NostrManager,
    onStatus: @escaping (String, RelayStatus) -> Void
) -> () -> Void
```

### SwiftUI Helpers

#### `Subscription`

An `@MainActor ObservableObject` that wraps a subscription and exposes `@Published` events.

```swift
@MainActor
public final class Subscription: ObservableObject {
    @Published public private(set) var events: [NostrEvent] = []
    @Published public private(set) var isComplete: Bool = false
    public let id: String

    public init(id: String, manager: NostrManager)
}
```

#### `PublishTracker`

An `@MainActor ObservableObject` that tracks publish statuses.

```swift
@MainActor
public final class PublishTracker: ObservableObject {
    @Published public private(set) var statuses: [String: PublishStatus] = [:]
    public let publishId: String

    public init(publishId: String, manager: NostrManager)
}
```

## Examples

### Subscribe & display events in SwiftUI

```swift
import SwiftUI
import NipworkerSwift

@MainActor
class FeedViewModel: ObservableObject {
    let manager = NostrManager()
    let sub: Subscription

    init() {
        sub = Subscription(id: "feed", manager: manager)
    }

    func start() async {
        let request = RequestObject(
            filters: [.init(kinds: [1], limit: 50)],
            relays: ["wss://relay.damus.io"]
        )
        _ = await manager.subscribe(
            subscriptionId: "feed",
            requests: [request]
        )
    }
}

struct FeedView: View {
    @StateObject var vm = FeedViewModel()

    var body: some View {
        List(vm.sub.events) { event in
            VStack(alignment: .leading) {
                Text(event.content)
                    .lineLimit(3)
                Text("Kind \(event.kind)")
                    .font(.caption)
                    .foregroundColor(.secondary)
            }
        }
        .task {
            await vm.start()
        }
    }
}
```

### Publish a signed event

```swift
await manager.setSigner(.privkey(secret: "your-hex-private-key"))

let template = EventTemplate(
    kind: 1,
    content: "Hello from Swift!",
    tags: []
)
let event = try await manager.signEvent(template)

let tracker = PublishTracker(publishId: "pub-1", manager: manager)

_ = await manager.publish(
    publishId: "pub-1",
    event: event,
    defaultRelays: ["wss://relay.damus.io"]
)

// tracker.statuses will update automatically via @Published
```

### Monitor relay status

```swift
let unsub = useRelayStatus(manager: manager) { relay, status in
    print("Relay \(relay): \(status)")
}
```

## Architecture

- **Rust FFI** — A static library (`libnipworker_native_ffi.a`) compiled from the `native-ffi` crate and bundled as an `XCFramework`.
- **FlatBuffers** — All messages crossing the Swift/Rust boundary are serialized with FlatBuffers for zero-allocation parsing on the Rust side.
- **Actor Isolation** — `NostrManager` is a Swift `actor` so all state mutation is serialized and safe to call from any concurrency context.
- **Subscription Buffers** — Events arrive from Rust into a shared `SubscriptionBuffer` (a raw pointer with a 4-byte header). Swift reads length-prefixed messages out of the buffer.

## Testing

Run the test suite from the `swift/` directory:

```bash
cd swift
swift test
```

The suite includes:
- **Unit tests** — `ArrayBufferReaderTests`, `NostrManagerTests`
- **Integration tests** — Live WebSocket subscription against `wss://relay.damus.io`

> Integration tests require an internet connection and may take a few seconds.

## Known Limitations

- **Swift 6 language mode warnings** — The package compiles and runs correctly, but produces a handful of concurrency warnings (e.g., accessing `handle`/`boxPtr` in the actor initializer). These are non-blocking.
- **FlatBuffers Object API** — Unavailable due to a `flatc` code-generation bug (`ParsedDataUnion` enum/struct name collision). All serialization uses the low-level builder API.
- **Native deployment target warnings** — Rebuild `NipworkerNativeFFI.xcframework` with `crates/native-ffi/ios/build-ios.sh`. The script passes explicit iOS and macOS minimum-version linker flags to Rust so the compiled static libraries match the Swift package deployment targets.

## License

Same as the parent NIPWorker project.
