import ActivityKit
import AppIntents
import Foundation

@available(iOS 17.0, *)
public struct OpenFilTerminalIntent: LiveActivityIntent {
    public static let title: LocalizedStringResource = "Open terminal"
    public static let description = IntentDescription("Open the followed Fil terminal.")
    public static let openAppWhenRun = true

    @Parameter(title: "Session ID")
    public var sessionId: String

    public init() {}

    public init(sessionId: String) {
        self.sessionId = sessionId
    }

    public func perform() async throws -> some IntentResult {
        guard let url = FilActivityURL.session(sessionId) else { return .result() }
        FilSharedStore.savePendingActivityURL(url)
        return .result()
    }
}

@available(iOS 17.0, *)
public struct StopFollowingFilIntent: LiveActivityIntent {
    public static let title: LocalizedStringResource = "Stop following"
    public static let description = IntentDescription("Remove the Fil Live Activity without closing the shell.")

    @Parameter(title: "Session ID")
    public var sessionId: String

    public init() {}

    public init(sessionId: String) {
        self.sessionId = sessionId
    }

    public func perform() async throws -> some IntentResult {
        for activity in Activity<FilActivityAttributes>.activities
        where activity.attributes.sessionId == sessionId {
            var finalState = activity.content.state
            finalState.status = .ended
            finalState.lastUpdatedAt = Date()
            await activity.end(
                ActivityContent(state: finalState, staleDate: nil),
                dismissalPolicy: .immediate
            )
        }
        return .result()
    }
}
