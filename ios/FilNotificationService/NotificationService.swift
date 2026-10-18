import Foundation
import UserNotifications

/// Gives each agent question one button per option. The hub marks question
/// pushes `mutable-content`; everything else passes through untouched.
final class NotificationService: UNNotificationServiceExtension {
    // Touched from the system callback queue and the expiry callback.
    private let lock = NSLock()
    private var contentHandler: ((UNNotificationContent) -> Void)?
    private var bestAttempt: UNMutableNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest,
        withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void
    ) {
        guard let payload = AgentPushPayload(userInfo: request.content.userInfo),
              payload.isQuestion,
              let content = request.content.mutableCopy() as? UNMutableNotificationContent else {
            contentHandler(request.content)
            return
        }
        lock.withLock {
            self.contentHandler = contentHandler
            self.bestAttempt = content
        }

        let options = payload.options
        nonisolated(unsafe) let service = self
        nonisolated(unsafe) let mutableContent = content
        UNUserNotificationCenter.current().getNotificationCategories { existing in
            let category = AgentNotificationCategory.category(for: options)
            UNUserNotificationCenter.current().setNotificationCategories(
                AgentNotificationCategory.merged(existing, adding: [category])
            )
            // Registration is asynchronous inside the system; the round trip
            // below makes sure it landed before the banner is built.
            UNUserNotificationCenter.current().getNotificationCategories { _ in
                mutableContent.categoryIdentifier = AgentNotificationCategory.identifier(for: options)
                service.finish()
            }
        }
    }

    override func serviceExtensionTimeWillExpire() {
        finish()
    }

    private func finish() {
        let pending: (((UNNotificationContent) -> Void), UNMutableNotificationContent)? = lock.withLock {
            guard let handler = contentHandler, let content = bestAttempt else { return nil }
            contentHandler = nil
            bestAttempt = nil
            return (handler, content)
        }
        guard let (handler, content) = pending else { return }
        handler(content)
    }
}
