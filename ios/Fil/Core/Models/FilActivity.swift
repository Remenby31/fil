import ActivityKit
import Foundation

struct FilActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        var sessionCount: Int
        var machineCount: Int
        var machineName: String
    }

    var startedAt: Date
}

@available(iOS 16.2, *)
enum FilActivityManager {
    static func startOrUpdate(sessionCount: Int, machineCount: Int, machineName: String) {
        let state = FilActivityAttributes.ContentState(
            sessionCount: sessionCount,
            machineCount: machineCount,
            machineName: machineName
        )

        let existingIds = Activity<FilActivityAttributes>.activities.map(\.id)

        if !existingIds.isEmpty {
            for id in existingIds {
                for activity in Activity<FilActivityAttributes>.activities where activity.id == id {
                    let content = ActivityContent(state: state, staleDate: nil)
                    nonisolated(unsafe) let a = activity
                    Task.detached { await a.update(content) }
                }
            }
        } else if sessionCount > 0 {
            let attributes = FilActivityAttributes(startedAt: Date())
            _ = try? Activity.request(
                attributes: attributes,
                content: .init(state: state, staleDate: nil),
                pushType: nil
            )
        }
    }

    static func endAll() {
        let finalState = FilActivityAttributes.ContentState(
            sessionCount: 0, machineCount: 0, machineName: ""
        )
        let content = ActivityContent(state: finalState, staleDate: nil)
        for activity in Activity<FilActivityAttributes>.activities {
            nonisolated(unsafe) let a = activity
            Task.detached { await a.end(content, dismissalPolicy: .immediate) }
        }
    }
}
