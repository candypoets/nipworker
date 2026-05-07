import Foundation

// MARK: - Callback-based subscription helpers

/// Subscribe with a callback closure. Returns an unsubscribe function.
/// Mirrors the JS `useSubscription` hook pattern.
public func useSubscription(
    manager: NostrManager,
    subscriptionId: String,
    requests: [RequestObject],
    callback: @escaping ([NostrEvent]) -> Void,
    options: SubscriptionConfig = SubscriptionConfig()
) -> () -> Void {
    var notificationToken: NSObjectProtocol?
    var lastReadPosition = 4
    var isActive = true

    Task {
        _ = await manager.subscribe(subscriptionId: subscriptionId, requests: requests, options: options)

        notificationToken = NotificationCenter.default.addObserver(
            forName: .nipworkerSubscriptionUpdated,
            object: nil,
            queue: .main
        ) { [weak manager] notification in
            guard isActive,
                  let manager = manager,
                  let updatedSubId = notification.userInfo?["subId"] as? String,
                  updatedSubId == subscriptionId else { return }

            Task {
                let result = await manager.readEvents(for: subscriptionId, from: lastReadPosition)
                lastReadPosition = result.newPosition
                if !result.events.isEmpty {
                    callback(result.events)
                }
            }
        }
    }

    return {
        isActive = false
        if let token = notificationToken {
            NotificationCenter.default.removeObserver(token)
        }
        Task {
            await manager.unsubscribe(subscriptionId: subscriptionId)
        }
    }
}

/// Publish with a callback for status updates. Returns an unsubscribe function.
/// Mirrors the JS `usePublish` hook pattern.
public func usePublish(
    manager: NostrManager,
    publishId: String,
    event: NostrEvent,
    callback: @escaping ([String: PublishStatus]) -> Void,
    defaultRelays: [String] = [],
    optimisticSubIds: [String] = []
) -> () -> Void {
    var notificationToken: NSObjectProtocol?
    var lastReadPosition = 4
    var isActive = true

    Task {
        _ = await manager.publish(
            publishId: publishId,
            event: event,
            defaultRelays: defaultRelays,
            optimisticSubIds: optimisticSubIds
        )

        notificationToken = NotificationCenter.default.addObserver(
            forName: .nipworkerSubscriptionUpdated,
            object: nil,
            queue: .main
        ) { [weak manager] notification in
            guard isActive,
                  let manager = manager,
                  let updatedPubId = notification.userInfo?["subId"] as? String,
                  updatedPubId == publishId else { return }

            Task {
                let result = await manager.readPublishStatuses(for: publishId, from: lastReadPosition)
                lastReadPosition = result.newPosition
                if !result.statuses.isEmpty {
                    callback(result.statuses)
                }
            }
        }
    }

    return {
        isActive = false
        if let token = notificationToken {
            NotificationCenter.default.removeObserver(token)
        }
    }
}

/// Relay status helper. Immediately calls handler with current statuses,
/// then subscribes to updates. Returns a stop function.
public func useRelayStatus(
    manager: NostrManager,
    onStatus: @escaping (String, RelayStatus) -> Void
) -> () -> Void {
    var notificationToken: NSObjectProtocol?
    var isActive = true

    Task {
        let statuses = await manager.getRelayStatuses()
        for (url, status) in statuses {
            onStatus(url, status)
        }

        notificationToken = NotificationCenter.default.addObserver(
            forName: .nipworkerRelayStatusUpdated,
            object: nil,
            queue: .main
        ) { notification in
            guard isActive,
                  let url = notification.userInfo?["url"] as? String,
                  let status = notification.userInfo?["status"] as? RelayStatus else { return }
            onStatus(url, status)
        }
    }

    return {
        isActive = false
        if let token = notificationToken {
            NotificationCenter.default.removeObserver(token)
        }
    }
}
