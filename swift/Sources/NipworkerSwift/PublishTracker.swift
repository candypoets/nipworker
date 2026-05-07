import Foundation
import Combine

@MainActor
public final class PublishTracker: ObservableObject {
    @Published public private(set) var statuses: [String: PublishStatus] = [:]

    public let publishId: String
    private let manager: NostrManager
    private var lastReadPosition: Int = 4
    private var notificationToken: NSObjectProtocol?

    public init(publishId: String, manager: NostrManager) {
        self.publishId = publishId
        self.manager = manager

        self.notificationToken = NotificationCenter.default.addObserver(
            forName: .nipworkerSubscriptionUpdated,
            object: nil,
            queue: .main
        ) { [weak self] notification in
            guard let self = self,
                  let subId = notification.userInfo?["subId"] as? String,
                  subId == self.publishId else { return }
            Task { await self.sync() }
        }
    }

    deinit {
        if let token = notificationToken {
            NotificationCenter.default.removeObserver(token)
        }
    }

    private func sync() async {
        let result = await manager.readPublishStatuses(for: publishId, from: lastReadPosition)
        lastReadPosition = result.newPosition
        for (url, status) in result.statuses {
            statuses[url] = status
        }
    }
}
