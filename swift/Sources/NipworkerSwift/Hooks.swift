import Foundation

public final class NipworkerHookHandle {
    private let onCancel: () -> Void
    private var isCancelled = false

    public init(_ onCancel: @escaping () -> Void) {
        self.onCancel = onCancel
    }

    public func cancel() {
        guard !isCancelled else { return }
        isCancelled = true
        onCancel()
    }

    deinit {
        cancel()
    }
}

// MARK: - Callback-based subscription helpers

/// Subscribe through the React Native shared runtime. Returns an unsubscribe function.
public func useSubscription(
    subscriptionId: String,
    requests: [RequestObject],
    callback: @escaping ([WorkerMessageView]) -> Void,
    options: SubscriptionConfig = SubscriptionConfig()
) -> () -> Void {
    useSubscription(
        manager: NostrManager.reactNativeShared(),
        subscriptionId: subscriptionId,
        requests: requests,
        callback: callback,
        options: options
    )
}

/// Subscribe with a callback closure. Returns an unsubscribe function.
/// Mirrors the JS `useSubscription` hook pattern.
public func useSubscription(
    manager: NostrManager,
    subscriptionId: String,
    requests: [RequestObject],
    callback: @escaping ([WorkerMessageView]) -> Void,
    options: SubscriptionConfig = SubscriptionConfig()
) -> () -> Void {
    var notificationToken: NSObjectProtocol?
    var lastReadPosition = 4
    var isActive = true
    let canonicalSubscriptionId = subscriptionId

    func readAvailableMessages() {
        Task {
            var position = lastReadPosition
            var allMessages: [WorkerMessageView] = []

            while true {
                let result = await manager.readWorkerMessages(for: canonicalSubscriptionId, from: position)
                if result.messages.isEmpty {
                    break
                }
                allMessages.append(contentsOf: result.messages)
                position = result.newPosition
            }

            lastReadPosition = position
            if isActive, !allMessages.isEmpty {
                callback(allMessages)
            }
        }
    }

    Task {
        notificationToken = NotificationCenter.default.addObserver(
            forName: .nipworkerSubscriptionUpdated,
            object: nil,
            queue: .main
        ) { notification in
            guard isActive,
                  let updatedSubId = notification.userInfo?["subId"] as? String,
                  updatedSubId == canonicalSubscriptionId else { return }

            readAvailableMessages()
        }

        _ = await manager.subscribe(subscriptionId: subscriptionId, requests: requests, options: options)
        readAvailableMessages()
    }

    return {
        isActive = false
        if let token = notificationToken {
            NotificationCenter.default.removeObserver(token)
        }
        Task {
            await manager.unsubscribe(subscriptionId: canonicalSubscriptionId)
        }
    }
}

/// Subscribe through the React Native shared runtime. Returns a cancellable handle.
public func useSubscriptionHandle(
    subscriptionId: String,
    requests: [RequestObject],
    callback: @escaping ([WorkerMessageView]) -> Void,
    options: SubscriptionConfig = SubscriptionConfig()
) -> NipworkerHookHandle {
    NipworkerHookHandle(
        useSubscription(
            subscriptionId: subscriptionId,
            requests: requests,
            callback: callback,
            options: options
        )
    )
}

/// Subscribe with a callback closure. Returns a cancellable handle.
/// This is the native component-friendly form of the JS `useSubscription` API.
public func useSubscriptionHandle(
    manager: NostrManager,
    subscriptionId: String,
    requests: [RequestObject],
    callback: @escaping ([WorkerMessageView]) -> Void,
    options: SubscriptionConfig = SubscriptionConfig()
) -> NipworkerHookHandle {
    NipworkerHookHandle(
        useSubscription(
            manager: manager,
            subscriptionId: subscriptionId,
            requests: requests,
            callback: callback,
            options: options
        )
    )
}

/// Publish through the React Native shared runtime. Returns an unsubscribe function.
public func usePublish(
    publishId: String,
    event: NostrEvent,
    callback: @escaping ([String: PublishStatus]) -> Void,
    defaultRelays: [String] = [],
    optimisticSubIds: [String] = []
) -> () -> Void {
    usePublish(
        manager: NostrManager.reactNativeShared(),
        publishId: publishId,
        event: event,
        callback: callback,
        defaultRelays: defaultRelays,
        optimisticSubIds: optimisticSubIds
    )
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
                var position = lastReadPosition
                var merged: [String: PublishStatus] = [:]

                while true {
                    let result = await manager.readPublishStatuses(for: publishId, from: position)
                    if result.statuses.isEmpty {
                        break
                    }
                    for (url, status) in result.statuses {
                        merged[url] = status
                    }
                    position = result.newPosition
                }

                lastReadPosition = position
                if !merged.isEmpty {
                    callback(merged)
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
            await manager.releasePublish(publishId: publishId)
        }
    }
}

/// Publish through the React Native shared runtime. Returns a cancellable handle.
public func usePublishHandle(
    publishId: String,
    event: NostrEvent,
    callback: @escaping ([String: PublishStatus]) -> Void,
    defaultRelays: [String] = [],
    optimisticSubIds: [String] = []
) -> NipworkerHookHandle {
    NipworkerHookHandle(
        usePublish(
            publishId: publishId,
            event: event,
            callback: callback,
            defaultRelays: defaultRelays,
            optimisticSubIds: optimisticSubIds
        )
    )
}

/// Publish with a callback for status updates. Returns a cancellable handle.
/// This is the native component-friendly form of the JS `usePublish` API.
public func usePublishHandle(
    manager: NostrManager,
    publishId: String,
    event: NostrEvent,
    callback: @escaping ([String: PublishStatus]) -> Void,
    defaultRelays: [String] = [],
    optimisticSubIds: [String] = []
) -> NipworkerHookHandle {
    NipworkerHookHandle(
        usePublish(
            manager: manager,
            publishId: publishId,
            event: event,
            callback: callback,
            defaultRelays: defaultRelays,
            optimisticSubIds: optimisticSubIds
        )
    )
}

/// Relay status helper using the React Native shared runtime.
public func useRelayStatus(
    onStatus: @escaping (String, RelayStatus) -> Void
) -> () -> Void {
    useRelayStatus(manager: NostrManager.reactNativeShared(), onStatus: onStatus)
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
