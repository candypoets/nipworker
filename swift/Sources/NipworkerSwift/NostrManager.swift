import Foundation
import Combine
import FlatBuffers

public extension Notification.Name {
    static let nipworkerSubscriptionUpdated = Notification.Name("nipworkerSubscriptionUpdated")
    static let nipworkerRelayStatusUpdated = Notification.Name("nipworkerRelayStatusUpdated")
    static let nipworkerAuthUpdated = Notification.Name("nipworkerAuthUpdated")

    static func nipworkerSubscriptionUpdated(subId: String) -> Notification.Name {
        Notification.Name("nipworkerSubscriptionUpdated.\(subId)")
    }
}

/// Weak reference box for the C callback
private final class ManagerBox {
    weak var manager: NostrManager?
    init(manager: NostrManager) { self.manager = manager }
}

public actor NostrManager {
    private static let sharedRuntimeNotification = "NipworkerRuntimeDataNotification"
    private static let sharedRuntimeDataKey = "data"

    private static var reactNativeSharedManager: NostrManager?

    private var handle: UnsafeMutableRawPointer?
    private let ownsHandle: Bool
    private var bufferViews: [String: SubscriptionBuffer] = [:]
    private var relayStatuses: [String: RelayStatus] = [:]
    private var activePubkey: String?
    private var pendingSession: (type: String, payload: Any)?
    private var sharedRuntimeObserver: NSObjectProtocol?

    private var signContinuation: CheckedContinuation<NostrEvent, Error>?
    private var pubkeyContinuation: CheckedContinuation<String, Error>?

    private var boxPtr: UnsafeMutablePointer<ManagerBox>

    public init(config: NostrManagerConfig = NostrManagerConfig()) {
        self.handle = nil
        self.ownsHandle = true
        self.boxPtr = UnsafeMutablePointer<ManagerBox>.allocate(capacity: 1)
        self.boxPtr.initialize(to: ManagerBox(manager: self))

        self.handle = nipworker_init({ userdata, ptr, len in
            guard let ptr = ptr else { return }
            let data = Data(bytes: ptr, count: len)
            nipworker_free_bytes(UnsafeMutablePointer(mutating: ptr), len)

            let box = userdata!.assumingMemoryBound(to: ManagerBox.self).pointee
            guard let manager = box.manager else { return }

            Task {
                await manager.handleNativeMessage(data)
            }
        }, self.boxPtr)
    }

    private init(borrowedHandle: UnsafeMutableRawPointer?) {
        self.handle = borrowedHandle
        self.ownsHandle = false
        self.boxPtr = UnsafeMutablePointer<ManagerBox>.allocate(capacity: 1)
        self.boxPtr.initialize(to: ManagerBox(manager: self))

        self.sharedRuntimeObserver = NotificationCenter.default.addObserver(
            forName: Notification.Name(rawValue: Self.sharedRuntimeNotification),
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let data = notification.userInfo?[Self.sharedRuntimeDataKey] as? Data else { return }
            Task { [weak self] in
                await self?.handleNativeMessage(data)
            }
        }
    }

    public static func reactNativeShared() -> NostrManager {
        if let manager = reactNativeSharedManager {
            return manager
        }
        guard let handle = nipworker_react_native_shared_handle_if_available() else {
            return NostrManager(borrowedHandle: nil)
        }
        let manager = NostrManager(borrowedHandle: handle)
        reactNativeSharedManager = manager
        return manager
    }

    deinit {
        if ownsHandle, let handle = handle {
            nipworker_deinit(handle)
        }
        if let observer = sharedRuntimeObserver {
            NotificationCenter.default.removeObserver(observer)
            sharedRuntimeObserver = nil
        }
        boxPtr.pointee.manager = nil
        boxPtr.deinitialize(count: 1)
        boxPtr.deallocate()
    }

    // MARK: - Subscribe

    public func subscribe(
        subscriptionId: String,
        requests: [RequestObject],
        options: SubscriptionConfig = SubscriptionConfig()
    ) -> SubscriptionBuffer {
        let subId = subscriptionId
        let existingRetained = subId.withCString { nipworker_retain_subscription(handle, $0) }
        if existingRetained, let buffer = bufferView(for: subId) {
            return buffer
        }

        let fbData = buildSubscribeMessage(subId: subId, requests: requests, options: options)
        let ok = fbData.withUnsafeBytes { bytes in
            nipworker_subscribe_message(
                handle,
                bytes.baseAddress?.assumingMemoryBound(to: UInt8.self),
                bytes.count
            )
        }

        guard ok, let buffer = bufferView(for: subId) else {
            return SubscriptionBuffer(capacity: 4)
        }
        return buffer
    }

    public func unsubscribe(subscriptionId: String) {
        subscriptionId.withCString { nipworker_release_subscription(handle, $0) }
    }

    // MARK: - Publish

    public func publish(
        publishId: String,
        event: NostrEvent,
        defaultRelays: [String] = [],
        optimisticSubIds: [String] = []
    ) -> SubscriptionBuffer {
        let fbData = buildPublishMessage(
            publishId: publishId,
            event: event,
            defaultRelays: defaultRelays,
            optimisticSubIds: optimisticSubIds
        )
        let ok = fbData.withUnsafeBytes { bytes in
            nipworker_publish_message(
                handle,
                bytes.baseAddress?.assumingMemoryBound(to: UInt8.self),
                bytes.count
            )
        }

        guard ok, let buffer = bufferView(for: publishId) else {
            return SubscriptionBuffer(capacity: 4)
        }
        return buffer
    }

    public func releasePublish(publishId: String) {
        publishId.withCString { nipworker_release_subscription(handle, $0) }
    }

    // MARK: - Signer

    public func setSigner(_ signer: SignerTypeName) {
        switch signer {
        case .pubkey(let pubkey):
            self.activePubkey = pubkey
            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .nipworkerAuthUpdated,
                    object: nil,
                    userInfo: ["pubkey": pubkey, "hasSigner": false]
                )
            }
        case .privkey(let secret):
            let fbData = buildSetPrivateKeyMessage(secret: secret)
            fbData.withUnsafeBytes { bytes in
                nipworker_handle_message(
                    handle,
                    bytes.baseAddress?.assumingMemoryBound(to: UInt8.self),
                    bytes.count
                )
            }
            Task {
                _ = try? await getPublicKey()
            }
        }
    }

    public func getPublicKey() async throws -> String {
        let fbData = buildGetPublicKeyMessage()
        fbData.withUnsafeBytes { bytes in
            nipworker_handle_message(
                handle,
                bytes.baseAddress?.assumingMemoryBound(to: UInt8.self),
                bytes.count
            )
        }

        return try await withCheckedThrowingContinuation { continuation in
            self.pubkeyContinuation = continuation
        }
    }

    public func signEvent(_ template: EventTemplate) async throws -> NostrEvent {
        let fbData = buildSignEventMessage(template: template)
        fbData.withUnsafeBytes { bytes in
            nipworker_handle_message(
                handle,
                bytes.baseAddress?.assumingMemoryBound(to: UInt8.self),
                bytes.count
            )
        }

        return try await withCheckedThrowingContinuation { continuation in
            self.signContinuation = continuation
        }
    }

    public func logout() {
        activePubkey = nil
        pendingSession = nil
    }

    // MARK: - Cleanup

    public func cleanup() {
        nipworker_cleanup_subscriptions(handle)
    }

    // MARK: - Read Events (called by Subscription)

    public func readEvents(for subId: String, from position: Int) -> (events: [NostrEvent], newPosition: Int) {
        guard let buffer = bufferView(for: subId) else { return ([], position) }
        let result = ArrayBufferReader.readMessages(buffer: buffer, lastReadPosition: position)
        let events = result.messages.compactMap { parseWorkerMessage($0) }
        return (events, result.newReadPosition)
    }

    public func readWorkerMessages(for subId: String, from position: Int) -> (messages: [WorkerMessageView], newPosition: Int) {
        guard let buffer = bufferView(for: subId) else { return ([], position) }
        let result = ArrayBufferReader.readMessages(buffer: buffer, lastReadPosition: position)
        let messages = result.messages.compactMap { WorkerMessageView($0) }
        return (messages, result.newReadPosition)
    }

    public func readPublishStatuses(for publishId: String, from position: Int) -> (statuses: [String: PublishStatus], newPosition: Int) {
        guard let buffer = bufferView(for: publishId) else { return ([:], position) }
        let result = ArrayBufferReader.readMessages(buffer: buffer, lastReadPosition: position)
        var statuses: [String: PublishStatus] = [:]
        for data in result.messages {
            if let (url, status) = parsePublishStatus(data) {
                statuses[url] = status
            }
        }
        return (statuses, result.newReadPosition)
    }

    public func getRelayStatuses() -> [String: RelayStatus] {
        return relayStatuses
    }

    public func getActivePubkey() -> String? {
        return activePubkey
    }

    // MARK: - Callback Handling

    private func handleNativeMessage(_ data: Data) {
        if let routeSubId = decodeRouteWakeFrame(data) {
            handleSubscriptionMessage(subId: routeSubId)
            return
        }
        guard data.count >= 4 else { return }
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        if rootOffset < 0 || Int(rootOffset) >= data.count { return }
        let workerMsg = nostr_fb_WorkerMessage(bb, o: rootOffset)
        let subId = workerMsg.subId ?? ""

        if subId == "crypto" {
            handleCryptoMessage(data)
        } else if subId.isEmpty {
            handleDirectResponse(data)
        } else {
            handleSubscriptionMessage(subId: subId)
        }
    }

    private func handleSubscriptionMessage(subId: String) {
        DispatchQueue.main.async {
            NotificationCenter.default.post(
                name: .nipworkerSubscriptionUpdated(subId: subId),
                object: nil,
                userInfo: ["subId": subId]
            )
        }
    }

    private func handleDirectResponse(_ payload: Data) {
        guard payload.count >= 4 else { return }

        let firstWord = Int(payload.withUnsafeBytes { $0.load(fromByteOffset: 0, as: UInt32.self).littleEndian })
        if firstWord > 0, payload.count == 4 + firstWord {
            _parseDirectResponse(payload.subdata(in: 4..<4 + firstWord))
        } else {
            _parseDirectResponse(payload)
        }
    }

    private func handleCryptoMessage(_ payload: Data) {
        _parseCryptoResponse(payload)
    }

    private func decodeRouteWakeFrame(_ data: Data) -> String? {
        guard data.count >= 8 else { return nil }
        guard [UInt8](data.prefix(4)) == [0x4e, 0x57, 0x52, 0x31] else { return nil }
        let subIdLength = Int(data.withUnsafeBytes {
            $0.load(fromByteOffset: 4, as: UInt32.self).littleEndian
        })
        guard subIdLength > 0, subIdLength == data.count - 8 else { return nil }
        return String(data: data.subdata(in: 8..<data.count), encoding: .utf8)
    }

    // MARK: - Helpers

    private func bufferView(for id: String) -> SubscriptionBuffer? {
        let nativePointer = id.withCString { nipworker_subscription_buffer_ptr(handle, $0) }
        let nativeLength = id.withCString { nipworker_subscription_buffer_len(handle, $0) }
        guard let nativePointer, nativeLength > 0 else {
            bufferViews.removeValue(forKey: id)
            return nil
        }
        if let existing = bufferViews[id],
           existing.pointer == UnsafeMutableRawPointer(nativePointer),
           existing.capacity == nativeLength {
            return existing
        }
        let buffer = SubscriptionBuffer(pointer: UnsafeMutableRawPointer(nativePointer), capacity: nativeLength)
        bufferViews[id] = buffer
        return buffer
    }

    public nonisolated func createShortId(_ input: String) -> String {
        if input.count < 64 { return input }
        var hash: Int32 = 0
        for char in input.utf16 {
            hash = (hash << 5) &- hash &+ Int32(char)
        }
        let shortId = String(Int(hash).magnitude, radix: 36)
        return String(shortId.prefix(63))
    }

    // MARK: - FlatBuffer Parsing

    private func parseWorkerMessage(_ data: Data) -> NostrEvent? {
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        let workerMsg = nostr_fb_WorkerMessage(bb, o: rootOffset)

        switch workerMsg.contentType {
        case .nostrevent:
            guard let event = workerMsg.content(type: nostr_fb_NostrEvent.self) else { return nil }
            return NostrEvent(
                id: event.id,
                pubkey: event.pubkey,
                kind: event.kind,
                content: event.content,
                tags: event.tags.map { $0.items.compactMap { $0 } },
                createdAt: Int(event.createdAt),
                sig: event.sig
            )
        case .parsedevent:
            guard let parsed = workerMsg.content(type: nostr_fb_ParsedEvent.self) else { return nil }
            return NostrEvent(
                id: parsed.id,
                pubkey: parsed.pubkey,
                kind: parsed.kind,
                content: "",
                tags: parsed.tags.map { $0.items.compactMap { $0 } },
                createdAt: Int(parsed.createdAt),
                sig: ""
            )
        case .raw:
            guard let raw = workerMsg.content(type: nostr_fb_Raw.self) else { return nil }
            return parseRawEventMessage(raw.raw)
        default:
            return nil
        }
    }

    private func parsePublishStatus(_ data: Data) -> (String, PublishStatus)? {
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        let workerMsg = nostr_fb_WorkerMessage(bb, o: rootOffset)

        switch workerMsg.contentType {
        case .connectionstatus:
            guard let status = workerMsg.content(type: nostr_fb_ConnectionStatus.self) else { return nil }
            let relayUrl: String = status.relayUrl
            let statusStr: String = status.status
            let message: String = status.message ?? ""
            switch statusStr {
            case "ok":
                return (relayUrl, .success(message))
            case "failed":
                return (relayUrl, .failed(message))
            default:
                return (relayUrl, .pending)
            }
        case .raw:
            guard let raw = workerMsg.content(type: nostr_fb_Raw.self) else { return nil }
            let text: String = raw.raw
            if text.contains("\"OK\"") || text.contains("\"ok\"") {
                return (text, .success(text))
            } else if text.contains("NOTICE") {
                return (text, .failed(text))
            }
            return nil
        default:
            return nil
        }
    }

    private func _parseDirectResponse(_ data: Data) {
        guard data.count >= 4 else { return }
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        guard rootOffset >= 0, Int(rootOffset) < data.count else { return }
        let workerMsg = nostr_fb_WorkerMessage(bb, o: rootOffset)

        switch workerMsg.contentType {
        case .pubkey:
            guard let pubkey = workerMsg.content(type: nostr_fb_Pubkey.self) else { return }
            let key: String = pubkey.pubkey
            activePubkey = key
            if let continuation = pubkeyContinuation {
                pubkeyContinuation = nil
                continuation.resume(returning: key)
            }
            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .nipworkerAuthUpdated,
                    object: nil,
                    userInfo: ["pubkey": key, "hasSigner": true]
                )
            }
        case .signedevent:
            guard let signed = workerMsg.content(type: nostr_fb_SignedEvent.self),
                  let event = signed.event else { return }
            let nostrEvent = NostrEvent(
                id: event.id,
                pubkey: event.pubkey,
                kind: event.kind,
                content: event.content,
                tags: event.tags.map { $0.items.compactMap { $0 } },
                createdAt: Int(event.createdAt),
                sig: event.sig
            )
            if let continuation = signContinuation {
                signContinuation = nil
                continuation.resume(returning: nostrEvent)
            }
        case .connectionstatus:
            guard let status = workerMsg.content(type: nostr_fb_ConnectionStatus.self) else { return }
            let url: String = status.relayUrl
            let statusStr: String = status.status
            let message: String = status.message ?? ""
            let relayStatus: RelayStatus
            switch statusStr {
            case "ok":
                relayStatus = .connected
            case "failed":
                relayStatus = .failed
            case "closed":
                relayStatus = .closed(message.isEmpty ? nil : message)
            default:
                relayStatus = .failed
            }
            relayStatuses[url] = relayStatus
            DispatchQueue.main.async {
                NotificationCenter.default.post(
                    name: .nipworkerRelayStatusUpdated,
                    object: nil,
                    userInfo: ["url": url, "status": relayStatus]
                )
            }
        default:
            break
        }
    }

    private func _parseCryptoResponse(_ data: Data) {
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        let workerMsg = nostr_fb_WorkerMessage(bb, o: rootOffset)

        guard workerMsg.contentType == .raw else { return }
        guard let raw = workerMsg.content(type: nostr_fb_Raw.self) else { return }
        let jsonStr: String = raw.raw

        guard let jsonData = jsonStr.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: jsonData) as? [String: Any] else { return }

        let op = json["op"] as? String
        let result = json["result"] as? String
        let error = json["error"] as? String

        if let error = error, !error.isEmpty {
            if let continuation = pubkeyContinuation {
                pubkeyContinuation = nil
                continuation.resume(throwing: NostrManagerError.cryptoError(error))
            }
            if let continuation = signContinuation {
                signContinuation = nil
                continuation.resume(throwing: NostrManagerError.cryptoError(error))
            }
            return
        }

        switch op {
        case "get_public_key":
            if let key = result {
                activePubkey = key
                if let continuation = pubkeyContinuation {
                    pubkeyContinuation = nil
                    continuation.resume(returning: key)
                }
                DispatchQueue.main.async {
                    NotificationCenter.default.post(
                        name: .nipworkerAuthUpdated,
                        object: nil,
                        userInfo: ["pubkey": key, "hasSigner": true]
                    )
                }
            }
        case "sign_event":
            if let eventJson = result,
               let eventData = eventJson.data(using: String.Encoding.utf8),
               let eventDict = try? JSONSerialization.jsonObject(with: eventData) as? [String: Any] {
                let event = parseEventDict(eventDict)
                if let continuation = signContinuation {
                    signContinuation = nil
                    continuation.resume(returning: event)
                }
            }
        default:
            break
        }
    }

    private func parseEventDict(_ dict: [String: Any]) -> NostrEvent {
        return NostrEvent(
            id: dict["id"] as? String ?? "",
            pubkey: dict["pubkey"] as? String ?? "",
            kind: uint16Value(dict["kind"]),
            content: dict["content"] as? String ?? "",
            tags: (dict["tags"] as? [[String]]) ?? [],
            createdAt: intValue(dict["created_at"]),
            sig: dict["sig"] as? String ?? ""
        )
    }

    private func parseRawEventMessage(_ raw: String) -> NostrEvent? {
        guard let data = raw.data(using: .utf8),
              let array = try? JSONSerialization.jsonObject(with: data) as? [Any],
              array.count >= 3,
              let messageType = array[0] as? String,
              messageType == "EVENT",
              let eventDict = array[2] as? [String: Any] else {
            return nil
        }
        return parseEventDict(eventDict)
    }

    private func uint16Value(_ value: Any?) -> UInt16 {
        if let value = value as? UInt16 { return value }
        if let value = value as? Int { return UInt16(value) }
        if let value = value as? NSNumber { return value.uint16Value }
        return 0
    }

    private func intValue(_ value: Any?) -> Int {
        if let value = value as? Int { return value }
        if let value = value as? NSNumber { return value.intValue }
        return 0
    }
}

public enum NostrManagerError: Error {
    case cryptoError(String)
}
