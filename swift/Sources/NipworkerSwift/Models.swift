import Foundation

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
    public var meshOnly: Bool?

    public init(
        ids: [String]? = nil,
        authors: [String]? = nil,
        kinds: [UInt16]? = nil,
        tags: [String: [String]]? = nil,
        since: Int? = nil,
        until: Int? = nil,
        limit: Int? = nil,
        search: String? = nil,
        relays: [String] = [],
        closeOnEOSE: Bool? = nil,
        cacheFirst: Bool? = nil,
        noCache: Bool? = nil,
        maxRelays: UInt16? = nil,
        meshOnly: Bool? = nil
    ) {
        self.ids = ids
        self.authors = authors
        self.kinds = kinds
        self.tags = tags
        self.since = since
        self.until = until
        self.limit = limit
        self.search = search
        self.relays = relays
        self.closeOnEOSE = closeOnEOSE
        self.cacheFirst = cacheFirst
        self.noCache = noCache
        self.maxRelays = maxRelays
        self.meshOnly = meshOnly
    }
}

public struct PipeConfig {
    public enum Kind {
        case muteFilter
        case parse
        case saveToDb
        case serializeEvents(subId: String)
        case kindFilter(kinds: [UInt16])
        case counter(kinds: [UInt16], pubkey: String)
        case npubLimiter(kind: UInt16, limitPerNpub: UInt32, maxTotalNpubs: UInt32)
        case proofVerification(maxProofs: UInt32)
    }
    public var kind: Kind
    public init(_ kind: Kind) { self.kind = kind }
}

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

    public init(
        pipeline: [PipeConfig]? = nil,
        closeOnEose: Bool = false,
        cacheFirst: Bool = true,
        timeoutMs: UInt64? = nil,
        maxEvents: UInt32? = nil,
        skipCache: Bool = false,
        force: Bool = false,
        bytesPerEvent: UInt32 = 3072,
        isSlow: Bool = false,
        pagination: String? = nil,
        cacheOnly: Bool = false
    ) {
        self.pipeline = pipeline
        self.closeOnEose = closeOnEose
        self.cacheFirst = cacheFirst
        self.timeoutMs = timeoutMs
        self.maxEvents = maxEvents
        self.skipCache = skipCache
        self.force = force
        self.bytesPerEvent = bytesPerEvent
        self.isSlow = isSlow
        self.pagination = pagination
        self.cacheOnly = cacheOnly
    }
}

public struct EventTemplate {
    public var kind: UInt16
    public var content: String
    public var tags: [[String]]

    public init(kind: UInt16, content: String, tags: [[String]] = []) {
        self.kind = kind
        self.content = content
        self.tags = tags
    }
}

public struct NostrEvent: Identifiable, Equatable, Sendable {
    public var id: String
    public var pubkey: String
    public var kind: UInt16
    public var content: String
    public var tags: [[String]]
    public var createdAt: Int
    public var sig: String

    public init(id: String, pubkey: String, kind: UInt16, content: String, tags: [[String]], createdAt: Int, sig: String) {
        self.id = id
        self.pubkey = pubkey
        self.kind = kind
        self.content = content
        self.tags = tags
        self.createdAt = createdAt
        self.sig = sig
    }
}

public struct NostrManagerConfig {
    public var logLevel: String?
    public var meshBLEEnabled: Bool

    public init(logLevel: String? = nil, meshBLEEnabled: Bool = false) {
        self.logLevel = logLevel
        self.meshBLEEnabled = meshBLEEnabled
    }
}

public enum SignerTypeName: Sendable {
    case pubkey(String)
    case privkey(String)
}

public enum RelayStatus: Equatable, Sendable {
    case connected
    case failed
    case closed(String?)
}

public enum PublishStatus: Equatable, Sendable {
    case pending
    case success(String)
    case failed(String)
}
