import XCTest
@testable import NipworkerSwift

final class ArrayBufferReaderTests: XCTestCase {
    func testInitializeBuffer() {
        let buffer = SubscriptionBuffer(capacity: 1024)
        ArrayBufferReader.initializeBuffer(buffer)
        XCTAssertEqual(ArrayBufferReader.getCurrentWritePosition(buffer: buffer), 4)
    }

    func testWriteAndReadMessage() {
        let buffer = SubscriptionBuffer(capacity: 1024)
        ArrayBufferReader.initializeBuffer(buffer)

        let data = Data("hello".utf8)
        let written = ArrayBufferReader.writeMessage(buffer: buffer, data: data)
        XCTAssertTrue(written)

        let result = ArrayBufferReader.readMessages(buffer: buffer, lastReadPosition: 4)
        XCTAssertTrue(result.hasNewData)
        XCTAssertEqual(result.messages.count, 1)
        XCTAssertEqual(result.messages[0], data)
    }

    func testBufferFull() {
        let buffer = SubscriptionBuffer(capacity: 8)
        ArrayBufferReader.initializeBuffer(buffer)

        let data = Data("hello".utf8) // 5 bytes + 4 byte header = 9, exceeds 8
        let written = ArrayBufferReader.writeMessage(buffer: buffer, data: data)
        XCTAssertFalse(written)
    }

    func testCalculateBufferSize() {
        let size = ArrayBufferReader.calculateBufferSize(totalEventLimit: 10, bytesPerEvent: 100)
        XCTAssertEqual(size, 4 + 1000 + 250)
    }
}
