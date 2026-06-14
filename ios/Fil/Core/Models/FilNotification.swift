import Foundation
import UserNotifications

enum FilNotificationType: String, Codable {
    case commandFinished
    case promptWaiting
    case commandError
}

struct FilNotification: Codable {
    let type: FilNotificationType
    let sessionId: String
    let deviceName: String
    let command: String?
    let exitCode: Int?
    let message: String?
}

enum NotificationManager {
    static func requestPermission() async -> Bool {
        do {
            return try await UNUserNotificationCenter.current()
                .requestAuthorization(options: [.alert, .sound, .badge])
        } catch {
            return false
        }
    }

    static func scheduleLocal(_ notification: FilNotification) {
        let content = UNMutableNotificationContent()
        content.sound = .default

        switch notification.type {
        case .commandFinished:
            content.title = "Command finished"
            content.body = "\(notification.command ?? "Process") completed on \(notification.deviceName)"
            content.categoryIdentifier = "COMMAND_FINISHED"

        case .promptWaiting:
            content.title = "Waiting for input"
            content.body = "\(notification.command ?? "Session") needs your attention on \(notification.deviceName)"
            content.categoryIdentifier = "PROMPT_WAITING"
            content.interruptionLevel = .timeSensitive

        case .commandError:
            content.title = "Command failed"
            let exitStr = notification.exitCode.map { " (exit \($0))" } ?? ""
            content.body = "\(notification.command ?? "Process") failed\(exitStr) on \(notification.deviceName)"
            content.categoryIdentifier = "COMMAND_ERROR"
        }

        content.userInfo = [
            "sessionId": notification.sessionId,
            "type": notification.type.rawValue,
        ]

        let request = UNNotificationRequest(
            identifier: UUID().uuidString,
            content: content,
            trigger: nil
        )

        UNUserNotificationCenter.current().add(request)
    }

    static func registerCategories() {
        let openAction = UNNotificationAction(
            identifier: "OPEN_SESSION",
            title: "Open",
            options: [.foreground]
        )

        let categories: [UNNotificationCategory] = [
            UNNotificationCategory(identifier: "COMMAND_FINISHED", actions: [openAction], intentIdentifiers: []),
            UNNotificationCategory(identifier: "PROMPT_WAITING", actions: [openAction], intentIdentifiers: []),
            UNNotificationCategory(identifier: "COMMAND_ERROR", actions: [openAction], intentIdentifiers: []),
        ]

        UNUserNotificationCenter.current().setNotificationCategories(Set(categories))
    }
}
