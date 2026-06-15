import ActivityKit
import SwiftUI
import WidgetKit

struct FilActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        var sessionCount: Int
        var machineCount: Int
        var machineName: String
    }

    var startedAt: Date
}

struct FilLiveActivity: Widget {
    let green = Color(red: 0, green: 0.83, blue: 0.67)
    let bg = Color(red: 0.039, green: 0.039, blue: 0.059)

    var body: some WidgetConfiguration {
        ActivityConfiguration(for: FilActivityAttributes.self) { context in
            // Lock Screen
            HStack(spacing: 12) {
                Image(systemName: "terminal")
                    .font(.system(size: 16))
                    .foregroundStyle(green)

                VStack(alignment: .leading, spacing: 2) {
                    Text("\(context.state.sessionCount) terminal\(context.state.sessionCount == 1 ? "" : "s") active")
                        .font(.system(size: 14, weight: .medium))
                        .foregroundStyle(.white)

                    Text(context.state.machineName)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.5))
                }

                Spacer()

                Circle()
                    .fill(green)
                    .frame(width: 8, height: 8)
            }
            .padding(16)
            .activityBackgroundTint(bg)

        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 6) {
                        Image(systemName: "terminal")
                            .foregroundStyle(green)
                        Text("\(context.state.sessionCount) active")
                            .font(.system(size: 13, weight: .medium))
                            .foregroundStyle(.white)
                    }
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Circle()
                        .fill(green)
                        .frame(width: 8, height: 8)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    Text(context.state.machineName)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.5))
                }
            } compactLeading: {
                Image(systemName: "terminal")
                    .foregroundStyle(green)
            } compactTrailing: {
                Text("\(context.state.sessionCount)")
                    .font(.system(size: 14, weight: .bold, design: .monospaced))
                    .foregroundStyle(green)
            } minimal: {
                Image(systemName: "terminal")
                    .foregroundStyle(green)
            }
        }
    }
}
