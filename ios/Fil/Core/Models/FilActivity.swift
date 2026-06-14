import ActivityKit
import Foundation

struct FilActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        var elapsedSeconds: Int
        var status: String
    }

    var command: String
    var deviceName: String
    var sessionId: String
}

@available(iOS 16.2, *)
enum FilActivityManager: Sendable {
    static func startActivity(
        command: String,
        deviceName: String,
        sessionId: String
    ) -> Activity<FilActivityAttributes>? {
        let attributes = FilActivityAttributes(
            command: command,
            deviceName: deviceName,
            sessionId: sessionId
        )

        let initialState = FilActivityAttributes.ContentState(
            elapsedSeconds: 0,
            status: "running"
        )

        do {
            return try Activity.request(
                attributes: attributes,
                content: .init(state: initialState, staleDate: nil),
                pushType: nil
            )
        } catch {
            return nil
        }
    }

    static func updateActivity(
        _ activityId: String,
        elapsedSeconds: Int
    ) async {
        let state = FilActivityAttributes.ContentState(
            elapsedSeconds: elapsedSeconds,
            status: "running"
        )
        for activity in Activity<FilActivityAttributes>.activities where activity.id == activityId {
            await activity.update(.init(state: state, staleDate: nil))
        }
    }

    static func endActivity(
        _ activityId: String,
        exitCode: Int
    ) async {
        let finalState = FilActivityAttributes.ContentState(
            elapsedSeconds: 0,
            status: exitCode == 0 ? "completed" : "failed"
        )
        for activity in Activity<FilActivityAttributes>.activities where activity.id == activityId {
            await activity.end(.init(state: finalState, staleDate: nil), dismissalPolicy: .after(.now + 5))
        }
    }
}
