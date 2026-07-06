import Foundation

/// Wraps a raw mutable buffer allocated with UnsafeMutablePointer.
/// Buffer format:
///   [0-3]:  write position (UInt32 little-endian)
///   [4..]:  [4-byte len][FlatBuffer payload]...
public final class SubscriptionBuffer {
    public let pointer: UnsafeMutableRawPointer
    public let capacity: Int
    private let ownsMemory: Bool

    public init(capacity: Int) {
        self.pointer = .allocate(byteCount: capacity, alignment: MemoryLayout<UInt32>.alignment)
        self.capacity = capacity
        self.ownsMemory = true
        // Initialize write position to 4 (right after header)
        pointer.storeBytes(of: UInt32(4).littleEndian, toByteOffset: 0, as: UInt32.self)
    }

    public init(pointer: UnsafeMutableRawPointer, capacity: Int) {
        self.pointer = pointer
        self.capacity = capacity
        self.ownsMemory = false
    }

    deinit {
        if ownsMemory {
            pointer.deallocate()
        }
    }
}

public enum ArrayBufferReader {
    public static func initializeBuffer(_ buffer: SubscriptionBuffer) {
        buffer.pointer.storeBytes(of: UInt32(4).littleEndian, toByteOffset: 0, as: UInt32.self)
    }

    public static func writeMessage(buffer: SubscriptionBuffer, data: Data) -> Bool {
        let currentWritePosition = readWritePosition(buffer)
        let requiredSpace = 4 + data.count
        guard currentWritePosition + requiredSpace <= buffer.capacity else {
            return false
        }

        buffer.pointer.storeBytes(
            of: UInt32(data.count).littleEndian,
            toByteOffset: currentWritePosition,
            as: UInt32.self
        )
        data.copyBytes(to: buffer.pointer.advanced(by: currentWritePosition + 4).assumingMemoryBound(to: UInt8.self), count: data.count)

        let newPosition = currentWritePosition + requiredSpace
        buffer.pointer.storeBytes(of: UInt32(newPosition).littleEndian, toByteOffset: 0, as: UInt32.self)
        return true
    }

    public static func writeBatchedData(buffer: SubscriptionBuffer, data: Data, debugId: String? = nil) -> Bool {
        let currentWritePosition = readWritePosition(buffer)
        guard currentWritePosition + data.count <= buffer.capacity else {
            return false
        }

        data.copyBytes(to: buffer.pointer.advanced(by: currentWritePosition).assumingMemoryBound(to: UInt8.self), count: data.count)

        let newPosition = currentWritePosition + data.count
        buffer.pointer.storeBytes(of: UInt32(newPosition).littleEndian, toByteOffset: 0, as: UInt32.self)
        return true
    }

    public static func readMessages(
        buffer: SubscriptionBuffer,
        lastReadPosition: Int = 0
    ) -> (messages: [Data], newReadPosition: Int, hasNewData: Bool) {
        let currentWritePosition = readWritePosition(buffer)
        let dataStartOffset = 4
        var currentPos = max(lastReadPosition, dataStartOffset)

        guard currentWritePosition > currentPos else {
            return ([], currentPos, false)
        }

        var messages: [Data] = []
        let maxMessages = 128

        while currentPos < currentWritePosition && messages.count < maxMessages {
            guard currentPos + 4 <= currentWritePosition else { break }

            let eventLength = Int(
                UInt32(buffer.pointer.load(fromByteOffset: currentPos, as: UInt8.self)) |
                (UInt32(buffer.pointer.load(fromByteOffset: currentPos + 1, as: UInt8.self)) << 8) |
                (UInt32(buffer.pointer.load(fromByteOffset: currentPos + 2, as: UInt8.self)) << 16) |
                (UInt32(buffer.pointer.load(fromByteOffset: currentPos + 3, as: UInt8.self)) << 24)
            )
            let payloadStart = currentPos + 4
            let nextPos = payloadStart + eventLength

            guard nextPos <= currentWritePosition else { break }

            let slice = Data(bytes: buffer.pointer.advanced(by: payloadStart), count: eventLength)
            messages.append(slice)
            currentPos = nextPos
        }

        return (messages, currentPos, messages.count > 0)
    }

    public static func readAllMessages(buffer: SubscriptionBuffer) -> (messages: [Data], totalMessages: Int) {
        let result = readMessages(buffer: buffer, lastReadPosition: 0)
        return (result.messages, result.messages.count)
    }

    public static func getCurrentWritePosition(buffer: SubscriptionBuffer) -> Int {
        return readWritePosition(buffer)
    }

    public static func hasNewData(buffer: SubscriptionBuffer, lastReadPosition: Int) -> Bool {
        let currentWritePosition = readWritePosition(buffer)
        return currentWritePosition > max(lastReadPosition, 4)
    }

    public static func calculateBufferSize(totalEventLimit: Int = 100, bytesPerEvent: Int = 3072) -> Int {
        let headerSize = 4
        let dataSize = totalEventLimit * bytesPerEvent
        let overhead = Int(Double(dataSize) * 0.25)
        return headerSize + dataSize + overhead
    }

    private static func readWritePosition(_ buffer: SubscriptionBuffer) -> Int {
        return Int(
            UInt32(buffer.pointer.load(fromByteOffset: 0, as: UInt8.self)) |
            (UInt32(buffer.pointer.load(fromByteOffset: 1, as: UInt8.self)) << 8) |
            (UInt32(buffer.pointer.load(fromByteOffset: 2, as: UInt8.self)) << 16) |
            (UInt32(buffer.pointer.load(fromByteOffset: 3, as: UInt8.self)) << 24)
        )
    }
}
