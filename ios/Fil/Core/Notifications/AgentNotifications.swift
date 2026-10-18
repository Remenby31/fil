import Foundation
import OSLog
import UIKit
import UserNotifications

/// SwiftUI has no hook for APNs registration or for a notification delegate
/// that must exist before launch finishes, so this adaptor does only that.
final class FilAppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        AgentNotifications.shared.install()
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        AgentNotifications.shared.didRegister(deviceToken: deviceToken)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        Logger(subsystem: "sh.fil.app", category: "AgentNotifications")
            .error("APNs registration failed: \(error.localizedDescription, privacy: .public)")
    }
}

/// Notifications sent by coding agents through `fil mcp`: permission, push
/// token registration, deep links into the agent's terminal, and answers.
@MainActor
final class AgentNotifications: NSObject, UNUserNotificationCenterDelegate {
    static let shared = AgentNotifications()

    nonisolated private static let deviceTokenKey = "agentNotificationDeviceToken"
    private let logger = Logger(subsystem: "sh.fil.app", category: "AgentNotifications")
    private var center: UNUserNotificationCenter { .current() }

    func install() {
        center.delegate = self
        // Merge, never replace: the service extension may have registered
        // categories for questions that are still on screen.
        center.getNotificationCategories { existing in
            UNUserNotificationCenter.current().setNotificationCategories(
                AgentNotificationCategory.merged(existing, adding: AgentNotificationCategory.staticCategories)
            )
        }
    }

    /// Run when a signed-in user reaches the main screen. Asks for permission
    /// once, and only when the hub can actually deliver agent notifications.
    func activate() async {
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .authorized, .provisional, .ephemeral:
            UIApplication.shared.registerForRemoteNotifications()
        case .notDetermined:
            guard await hubSupportsAgentNotifications() else { return }
            let granted = (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
            if granted { UIApplication.shared.registerForRemoteNotifications() }
        default:
            break
        }
    }

    func didRegister(deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        UserDefaults.standard.set(hex, forKey: Self.deviceTokenKey)
        guard let token = TokenStorage.loadToken(),
              let request = AgentNotificationAPI.registerRequest(
                  hubURL: TokenStorage.loadHubUrl(), token: token, deviceToken: hex
              ) else { return }
        Task {
            do {
                let (_, response) = try await URLSession.shared.data(for: request)
                if let status = (response as? HTTPURLResponse)?.statusCode, !(200...299).contains(status) {
                    logger.error("Push token registration failed with HTTP \(status)")
                }
            } catch {
                logger.error("Push token registration failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// Call before the account token is cleared, so the old account stops
    /// paging this phone.
    nonisolated static func unregisterBeforeLogout() {
        guard let deviceToken = UserDefaults.standard.string(forKey: deviceTokenKey),
              let token = TokenStorage.loadToken(),
              let request = AgentNotificationAPI.unregisterRequest(
                  hubURL: TokenStorage.loadHubUrl(), token: token, deviceToken: deviceToken
              ) else { return }
        Task { _ = try? await URLSession.shared.data(for: request) }
    }

    private func hubSupportsAgentNotifications() async -> Bool {
        guard let url = URL(string: TokenStorage.loadHubUrl())?.appendingPathComponent("health"),
              let (data, _) = try? await URLSession.shared.data(from: url),
              let health = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return false }
        return health["agent_notifications"] as? Bool == true
    }

    // MARK: - UNUserNotificationCenterDelegate

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse
    ) async {
        let content = response.notification.request.content
        guard let payload = AgentPushPayload(userInfo: content.userInfo) else { return }
        let prompt = AgentQuestionPrompt(title: content.title, body: content.body, payload: payload)
        await handle(prompt: prompt, action: response.actionIdentifier)
    }

    private func handle(prompt: AgentQuestionPrompt, action: String) async {
        let payload = prompt.payload
        if let index = AgentNotificationCategory.optionIndex(fromAction: action),
           payload.options.indices.contains(index) {
            // Answered from the banner; the app may only be running in the background.
            let outcome = await send(answer: payload.options[index], to: payload.notificationId)
            if outcome != .delivered { postFailure(outcome, for: prompt) }
            return
        }
        guard action == UNNotificationDefaultActionIdentifier else { return }
        if let sessionId = payload.sessionId, let url = FilActivityURL.session(sessionId) {
            FilApp.store.send(.openURL(url))
        }
        if payload.isQuestion {
            // Let the terminal presentation settle before showing the choices.
            try? await Task.sleep(for: .milliseconds(700))
            present(prompt)
        }
    }

    // MARK: - Answering

    private func send(answer: String, to notificationId: String) async -> AgentAnswerOutcome {
        guard let token = TokenStorage.loadToken(),
              let request = AgentNotificationAPI.answerRequest(
                  hubURL: TokenStorage.loadHubUrl(), token: token,
                  notificationId: notificationId, answer: answer
              ) else { return .failed }
        do {
            let (_, response) = try await URLSession.shared.data(for: request)
            return AgentAnswerOutcome(status: (response as? HTTPURLResponse)?.statusCode ?? 0)
        } catch {
            logger.error("Sending an answer failed: \(error.localizedDescription, privacy: .public)")
            return .failed
        }
    }

    private func present(_ prompt: AgentQuestionPrompt) {
        if let expiresAt = prompt.payload.expiresAt, expiresAt <= Date() {
            showMessage(AgentAnswerOutcome.closed.message)
            return
        }
        let alert = UIAlertController(
            title: prompt.title.isEmpty ? String(localized: "Question") : prompt.title,
            message: prompt.body,
            preferredStyle: .alert
        )
        for option in prompt.payload.options {
            alert.addAction(UIAlertAction(title: option, style: .default) { _ in
                Task { @MainActor in
                    let outcome = await AgentNotifications.shared.send(
                        answer: option, to: prompt.payload.notificationId
                    )
                    if outcome == .delivered {
                        UINotificationFeedbackGenerator().notificationOccurred(.success)
                    } else {
                        AgentNotifications.shared.showMessage(outcome.message)
                    }
                }
            })
        }
        alert.addAction(UIAlertAction(title: String(localized: "Later"), style: .cancel))
        topViewController()?.present(alert, animated: true)
    }

    private func showMessage(_ message: String) {
        let alert = UIAlertController(title: nil, message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: String(localized: "OK"), style: .default))
        topViewController()?.present(alert, animated: true)
    }

    /// The terminal is itself presented modally; an alert must go on top of it.
    private func topViewController() -> UIViewController? {
        let scene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .first { $0.activationState == .foregroundActive } ?? UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }.first
        var top = scene?.windows.first { $0.isKeyWindow }?.rootViewController
        while let presented = top?.presentedViewController, !presented.isBeingDismissed {
            top = presented
        }
        return top
    }

    private func postFailure(_ outcome: AgentAnswerOutcome, for prompt: AgentQuestionPrompt) {
        let content = UNMutableNotificationContent()
        content.title = String(localized: "Answer not sent")
        content.body = outcome.message
        content.sound = .default
        // Reuse the question payload so tapping retries in the app.
        content.userInfo = prompt.payload.userInfo
        content.categoryIdentifier = AgentNotificationCategory.ask
        content.threadIdentifier = prompt.payload.sessionId ?? "fil.agent"
        center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }
}

struct AgentQuestionPrompt: Sendable {
    let title: String
    let body: String
    let payload: AgentPushPayload
}

enum AgentAnswerOutcome: Equatable {
    case delivered
    /// Expired, already answered from another device, or not this account's.
    case closed
    case macOffline
    case failed

    init(status: Int) {
        switch status {
        case 200...299: self = .delivered
        case 404, 410: self = .closed
        case 503: self = .macOffline
        default: self = .failed
        }
    }

    var message: String {
        switch self {
        case .delivered: String(localized: "Answer sent.")
        case .closed: String(localized: "This question is no longer open.")
        case .macOffline: String(localized: "Your Mac is offline. Try again when it reconnects.")
        case .failed: String(localized: "Fil could not reach the hub. Try again from the app.")
        }
    }
}

extension AgentPushPayload {
    var userInfo: [String: Any] {
        var fil: [String: Any] = [
            "kind": kind.rawValue,
            "notification_id": notificationId,
            "options": options,
        ]
        if let sessionId { fil["session_id"] = sessionId }
        if let expiresAt { fil["expires_at"] = Int(expiresAt.timeIntervalSince1970) }
        return ["fil": fil]
    }
}

/// Request builders, kept pure for tests. The account credential only ever
/// goes to an HTTPS hub (or loopback in a simulator debug build).
enum AgentNotificationAPI {
    static func registerRequest(hubURL: String, token: String, deviceToken: String) -> URLRequest? {
        #if DEBUG
        let environment = "sandbox"
        #else
        let environment = "production"
        #endif
        return request(hubURL: hubURL, path: "push-tokens", method: "POST", token: token,
                       body: ["token": deviceToken, "environment": environment])
    }

    static func unregisterRequest(hubURL: String, token: String, deviceToken: String) -> URLRequest? {
        guard deviceToken.allSatisfy(\.isHexDigit) else { return nil }
        return request(hubURL: hubURL, path: "push-tokens/\(deviceToken)", method: "DELETE", token: token, body: nil)
    }

    static func answerRequest(hubURL: String, token: String, notificationId: String, answer: String) -> URLRequest? {
        let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-"))
        guard !notificationId.isEmpty, notificationId.count <= 64,
              notificationId.unicodeScalars.allSatisfy(allowed.contains) else { return nil }
        return request(hubURL: hubURL, path: "agent-notifications/\(notificationId)/answer",
                       method: "POST", token: token, body: ["answer": answer])
    }

    private static func request(
        hubURL: String, path: String, method: String, token: String, body: [String: String]?
    ) -> URLRequest? {
        guard !token.isEmpty, token.utf8.allSatisfy({ (0x21...0x7e).contains($0) }),
              var components = URLComponents(string: hubURL),
              let host = components.host, !host.isEmpty,
              components.user == nil, components.password == nil else { return nil }
        #if DEBUG && targetEnvironment(simulator)
        let localHTTP = components.scheme?.lowercased() == "http"
            && ["localhost", "127.0.0.1", "::1", "[::1]"].contains(host.lowercased())
        #else
        let localHTTP = false
        #endif
        guard components.scheme?.lowercased() == "https" || localHTTP else { return nil }
        while components.path.hasSuffix("/") { components.path.removeLast() }
        components.path += "/" + path
        components.query = nil
        components.fragment = nil
        guard let url = components.url else { return nil }
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = 20
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try? JSONSerialization.data(withJSONObject: body)
        }
        return request
    }
}
