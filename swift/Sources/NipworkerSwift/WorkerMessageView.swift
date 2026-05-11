import Foundation
import FlatBuffers

public struct WorkerMessageView: Sendable {
    public let data: Data

    public init?(_ data: Data) {
        guard data.count >= 4 else { return nil }
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        guard rootOffset >= 0, Int(rootOffset) < data.count else { return nil }
        self.data = data
    }

    public var message: nostr_fb_WorkerMessage {
        let bb = ByteBuffer(data: data)
        let rootOffset = bb.read(def: Int32.self, position: 0)
        return nostr_fb_WorkerMessage(bb, o: rootOffset)
    }

    public var contentType: nostr_fb_Message {
        message.contentType
    }

    public var messageType: nostr_fb_MessageType {
        message.type
    }

    public var parsedEvent: nostr_fb_ParsedEvent? {
        guard contentType == .parsedevent else { return nil }
        return message.content(type: nostr_fb_ParsedEvent.self)
    }

    public var parsedType: nostr_fb_ParsedData? {
        parsedEvent?.parsedType
    }

    public func parsedContent<T: FlatbuffersInitializable>(as type: T.Type, when parsedType: nostr_fb_ParsedData) -> T? {
        guard let event = parsedEvent, event.parsedType == parsedType else { return nil }
        return event.parsed(type: type)
    }

    public var kind0: nostr_fb_Kind0Parsed? {
        parsedContent(as: nostr_fb_Kind0Parsed.self, when: .kind0parsed)
    }

    public var kind1: nostr_fb_Kind1Parsed? {
        parsedContent(as: nostr_fb_Kind1Parsed.self, when: .kind1parsed)
    }

    public var kind3: nostr_fb_Kind3Parsed? {
        parsedContent(as: nostr_fb_Kind3Parsed.self, when: .kind3parsed)
    }

    public var kind4: nostr_fb_Kind4Parsed? {
        parsedContent(as: nostr_fb_Kind4Parsed.self, when: .kind4parsed)
    }

    public var kind6: nostr_fb_Kind6Parsed? {
        parsedContent(as: nostr_fb_Kind6Parsed.self, when: .kind6parsed)
    }

    public var kind7: nostr_fb_Kind7Parsed? {
        parsedContent(as: nostr_fb_Kind7Parsed.self, when: .kind7parsed)
    }

    public var kind17: nostr_fb_Kind17Parsed? {
        parsedContent(as: nostr_fb_Kind17Parsed.self, when: .kind17parsed)
    }

    public var kind20: nostr_fb_Kind20Parsed? {
        parsedContent(as: nostr_fb_Kind20Parsed.self, when: .kind20parsed)
    }

    public var kind22: nostr_fb_Kind22Parsed? {
        parsedContent(as: nostr_fb_Kind22Parsed.self, when: .kind22parsed)
    }

    public var kind1111: nostr_fb_Kind1111Parsed? {
        parsedContent(as: nostr_fb_Kind1111Parsed.self, when: .kind1111parsed)
    }

    public var kind1311: nostr_fb_Kind1311Parsed? {
        parsedContent(as: nostr_fb_Kind1311Parsed.self, when: .kind1311parsed)
    }

    public var kind1018: nostr_fb_Kind1018Parsed? {
        parsedContent(as: nostr_fb_Kind1018Parsed.self, when: .kind1018parsed)
    }

    public var kind1068: nostr_fb_Kind1068Parsed? {
        parsedContent(as: nostr_fb_Kind1068Parsed.self, when: .kind1068parsed)
    }

    public var kind10002: nostr_fb_Kind10002Parsed? {
        parsedContent(as: nostr_fb_Kind10002Parsed.self, when: .kind10002parsed)
    }

    public var kind10019: nostr_fb_Kind10019Parsed? {
        parsedContent(as: nostr_fb_Kind10019Parsed.self, when: .kind10019parsed)
    }

    public var kind17375: nostr_fb_Kind17375Parsed? {
        parsedContent(as: nostr_fb_Kind17375Parsed.self, when: .kind17375parsed)
    }

    public var kind7374: nostr_fb_Kind7374Parsed? {
        parsedContent(as: nostr_fb_Kind7374Parsed.self, when: .kind7374parsed)
    }

    public var kind7375: nostr_fb_Kind7375Parsed? {
        parsedContent(as: nostr_fb_Kind7375Parsed.self, when: .kind7375parsed)
    }

    public var kind7376: nostr_fb_Kind7376Parsed? {
        parsedContent(as: nostr_fb_Kind7376Parsed.self, when: .kind7376parsed)
    }

    public var kind9321: nostr_fb_Kind9321Parsed? {
        parsedContent(as: nostr_fb_Kind9321Parsed.self, when: .kind9321parsed)
    }

    public var kind9735: nostr_fb_Kind9735Parsed? {
        parsedContent(as: nostr_fb_Kind9735Parsed.self, when: .kind9735parsed)
    }

    public var kind30023: nostr_fb_Kind30023Parsed? {
        parsedContent(as: nostr_fb_Kind30023Parsed.self, when: .kind30023parsed)
    }

    public var nostrEvent: nostr_fb_NostrEvent? {
        guard contentType == .nostrevent else { return nil }
        return message.content(type: nostr_fb_NostrEvent.self)
    }

    public var raw: nostr_fb_Raw? {
        guard contentType == .raw else { return nil }
        return message.content(type: nostr_fb_Raw.self)
    }

    public var countResponse: nostr_fb_CountResponse? {
        guard contentType == .countresponse else { return nil }
        return message.content(type: nostr_fb_CountResponse.self)
    }
}
