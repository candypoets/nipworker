import XCTest
@testable import NipworkerSwift

final class AuthSessionTests: XCTestCase {
    func testPrivateKeyAuthSessionRoundTripsThroughJSON() throws {
        let session = AuthSession(
            pubkey: "pubkey",
            signer: .privateKey("secret"),
            createdAt: Date(timeIntervalSince1970: 1),
            lastUsedAt: Date(timeIntervalSince1970: 2)
        )

        let data = try JSONEncoder().encode(session)
        let decoded = try JSONDecoder().decode(AuthSession.self, from: data)

        XCTAssertEqual(decoded, session)
    }
}
