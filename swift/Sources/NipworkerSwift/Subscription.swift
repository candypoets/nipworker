import Foundation
import Combine

@MainActor
public final class Subscription: ObservableObject {
    @Published public private(set) var events: [NostrEvent] = []
    @Published public private(set) var isComplete: Bool = false

    public let id: String
    private let manager: NostrManager
    private var lastReadPosition: Int = 4
    private var notificationToken: NSObjectProtocol?

    public init(id: String, manager: NostrManager) {
        self.id = id
        self.manager = manager

        self.notificationToken = NotificationCenter.default.addObserver(
            forName: .nipworkerSubscriptionUpdated,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self = self,
                  let subId = notification.userInfo?["subId"] as? String,
                  subId == self.id else { return }
            Task { await self.sync() }
        }
    }

    deinit {
        if let token = notificationToken {
            NotificationCenter.default.removeObserver(token)
        }
        Task { [id, manager] in
            await manager.unsubscribe(subscriptionId: id)
        }
    }

    private func sync() async {
        let result = await manager.readEvents(for: id, from: lastReadPosition)
        lastReadPosition = result.newPosition
        if !result.events.isEmpty {
            events.append(contentsOf: result.events)
        }
    }
}
