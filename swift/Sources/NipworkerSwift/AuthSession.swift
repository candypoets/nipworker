import Foundation

public enum AuthSigner: Codable, Equatable, Sendable {
    case privateKey(String)
}

public struct AuthSession: Codable, Equatable, Sendable {
    public var pubkey: String
    public var signer: AuthSigner
    public var createdAt: Date
    public var lastUsedAt: Date?

    public init(
        pubkey: String,
        signer: AuthSigner,
        createdAt: Date = Date(),
        lastUsedAt: Date? = nil
    ) {
        self.pubkey = pubkey
        self.signer = signer
        self.createdAt = createdAt
        self.lastUsedAt = lastUsedAt
    }
}
