import CryptoKit
import Foundation
import UserNotifications

/// The `fil` dictionary the hub puts next to `aps` in an agent push.
/// Compiled into the app and the notification service extension.
struct AgentPushPayload: Equatable, Sendable {
    enum Kind: String, Sendable {
        case notify
        case ask
    }

    let kind: Kind
    let notificationId: String
    let sessionId: String?
    let options: [String]
    let expiresAt: Date?

    var isQuestion: Bool { kind == .ask && !options.isEmpty }

    init?(userInfo: [AnyHashable: Any]) {
        guard let fil = userInfo["fil"] as? [String: Any],
              let kind = (fil["kind"] as? String).flatMap(Kind.init(rawValue:)),
              let notificationId = fil["notification_id"] as? String,
              !notificationId.isEmpty else { return nil }
        self.kind = kind
        self.notificationId = notificationId
        self.sessionId = (fil["session_id"] as? String).flatMap { $0.isEmpty ? nil : $0 }
        // Cap at four: one notification action per option.
        self.options = Array(((fil["options"] as? [String]) ?? []).prefix(4))
        self.expiresAt = (fil["expires_at"] as? NSNumber).map { Date(timeIntervalSince1970: $0.doubleValue) }
    }

    init(kind: Kind, notificationId: String, sessionId: String?, options: [String], expiresAt: Date?) {
        self.kind = kind
        self.notificationId = notificationId
        self.sessionId = sessionId
        self.options = options
        self.expiresAt = expiresAt
    }
}

/// Notification actions cannot be set per notification, only per category.
/// A question therefore gets a category derived from its options, registered
/// by the service extension just before the notification is shown.
enum AgentNotificationCategory {
    static let notify = "fil.agent.notify"
    /// Fallback when the service extension did not run: tap to answer in-app.
    static let ask = "fil.agent.ask"
    static let dynamicPrefix = "fil.agent.ask."
    static let optionActionPrefix = "fil.option."
    /// Old option sets are pruned so the category list cannot grow forever.
    static let maxDynamicCategories = 24

    static func identifier(for options: [String]) -> String {
        let digest = SHA256.hash(data: Data(options.joined(separator: "\u{1F}").utf8))
        return dynamicPrefix + digest.prefix(8).map { String(format: "%02x", $0) }.joined()
    }

    static func optionIndex(fromAction identifier: String) -> Int? {
        guard identifier.hasPrefix(optionActionPrefix) else { return nil }
        return Int(identifier.dropFirst(optionActionPrefix.count))
    }

    static func category(for options: [String]) -> UNNotificationCategory {
        let actions = options.prefix(4).enumerated().map { index, title in
            // Answering can approve a deploy: require an unlocked phone.
            UNNotificationAction(
                identifier: optionActionPrefix + String(index),
                title: title,
                options: [.authenticationRequired]
            )
        }
        return UNNotificationCategory(
            identifier: identifier(for: options),
            actions: actions,
            intentIdentifiers: [],
            options: []
        )
    }

    static var staticCategories: [UNNotificationCategory] {
        [
            UNNotificationCategory(identifier: notify, actions: [], intentIdentifiers: []),
            UNNotificationCategory(identifier: ask, actions: [], intentIdentifiers: []),
        ]
    }

    /// Existing categories plus `adding`, keeping only the newest dynamic ones.
    /// Categories that are not ours are left alone.
    static func merged(
        _ existing: Set<UNNotificationCategory>,
        adding: [UNNotificationCategory]
    ) -> Set<UNNotificationCategory> {
        let addedIds = Set(adding.map(\.identifier))
        var kept = existing.filter { !addedIds.contains($0.identifier) }
        let dynamic = kept.filter { $0.identifier.hasPrefix(dynamicPrefix) }
        let newDynamic = adding.filter { $0.identifier.hasPrefix(dynamicPrefix) }.count
        let overflow = dynamic.count + newDynamic - maxDynamicCategories
        if overflow > 0 {
            // A Set has no order; dropping arbitrary old option sets is fine,
            // delivered notifications keep working through the in-app prompt.
            for category in dynamic.prefix(overflow) { kept.remove(category) }
        }
        return kept.union(adding)
    }
}
