import ActivityKit
import SwiftUI
import WidgetKit

struct FilActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable {
        var elapsedSeconds: Int
        var status: String
    }

    var command: String
    var deviceName: String
    var sessionId: String
}

struct FilLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: FilActivityAttributes.self) { context in
            // Lock Screen / banner view
            HStack(spacing: 12) {
                Image(systemName: "terminal")
                    .font(.system(size: 16))
                    .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))

                VStack(alignment: .leading, spacing: 2) {
                    Text(context.attributes.command)
                        .font(.system(size: 14, weight: .medium, design: .monospaced))
                        .foregroundStyle(.white)

                    Text(context.attributes.deviceName)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.5))
                }

                Spacer()

                Text(formatDuration(context.state.elapsedSeconds))
                    .font(.system(size: 16, weight: .semibold, design: .monospaced))
                    .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))
            }
            .padding(16)
            .activityBackgroundTint(Color(red: 0.039, green: 0.039, blue: 0.059))

        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 6) {
                        Image(systemName: "terminal")
                            .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))
                        Text(context.attributes.command)
                            .font(.system(size: 13, weight: .medium, design: .monospaced))
                            .foregroundStyle(.white)
                    }
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(formatDuration(context.state.elapsedSeconds))
                        .font(.system(size: 14, weight: .semibold, design: .monospaced))
                        .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))
                }
                DynamicIslandExpandedRegion(.bottom) {
                    Text(context.attributes.deviceName)
                        .font(.system(size: 12))
                        .foregroundStyle(.white.opacity(0.5))
                }
            } compactLeading: {
                Image(systemName: "terminal")
                    .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))
            } compactTrailing: {
                Text(formatDuration(context.state.elapsedSeconds))
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))
            } minimal: {
                Image(systemName: "terminal")
                    .foregroundStyle(Color(red: 0, green: 0.83, blue: 0.67))
            }
        }
    }

    private func formatDuration(_ seconds: Int) -> String {
        let m = seconds / 60
        let s = seconds % 60
        return String(format: "%d:%02d", m, s)
    }
}
