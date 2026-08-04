import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

private enum FilLiveColors {
    static let green = Color(red: 0, green: 0.83, blue: 0.67)
    static let orange = Color(red: 1, green: 0.66, blue: 0.22)
    static let gray = Color(white: 0.52)
    static let background = Color(red: 0.039, green: 0.039, blue: 0.059)
}

private struct FilMonogram: View {
    var compact = false

    var body: some View {
        Text("fil.")
            .font(.system(size: compact ? 12 : 15, weight: .semibold, design: .rounded))
            .tracking(-0.7)
            .accessibilityLabel("Fil")
    }
}

private extension FilActivityStatus {
    var label: LocalizedStringResource {
        switch self {
        case .connected: "Connected"
        case .reconnecting: "Reconnecting…"
        case .stale: "Last seen"
        case .ended: "Session ended"
        }
    }

    var color: Color {
        switch self {
        case .connected: FilLiveColors.green
        case .reconnecting: FilLiveColors.orange
        case .stale, .ended: FilLiveColors.gray
        }
    }
}

struct FilLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: FilActivityAttributes.self) { context in
            FilLockScreenView(context: context)
                .widgetURL(FilActivityURL.session(context.attributes.sessionId))
                .activityBackgroundTint(FilLiveColors.background)
                .activitySystemActionForegroundColor(.white)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    HStack(spacing: 7) {
                        FilMonogram()
                            .foregroundStyle(context.state.status.color)
                        Text(context.attributes.machineName)
                            .font(.system(size: 13, weight: .semibold))
                            .lineLimit(1)
                    }
                }
                DynamicIslandExpandedRegion(.trailing) {
                    Text(timerInterval: context.attributes.startedAt...Date.distantFuture, countsDown: false)
                        .font(.system(size: 12, design: .monospaced))
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    VStack(alignment: .leading, spacing: 9) {
                        HStack {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(context.state.projectName)
                                    .font(.system(size: 14, weight: .medium))
                                    .lineLimit(1)
                                HStack(spacing: 5) {
                                    Circle()
                                        .fill(context.state.status.color)
                                        .frame(width: 6, height: 6)
                                    Text(context.state.status.label)
                                    if !context.state.shell.isEmpty {
                                        Text("· \(context.state.shell)")
                                    }
                                    if context.state.otherSessionCount > 0 {
                                        Text("· +\(context.state.otherSessionCount)")
                                    }
                                }
                                .font(.system(size: 11))
                                .foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 8)
                        }
                        HStack(spacing: 8) {
                            Button(intent: OpenFilTerminalIntent(sessionId: context.attributes.sessionId)) {
                                Label("Open terminal", systemImage: "arrow.up.forward.app")
                                    .font(.system(size: 12, weight: .semibold))
                            }
                            .buttonStyle(.borderedProminent)
                            .tint(FilLiveColors.green)

                            Button(intent: StopFollowingFilIntent(sessionId: context.attributes.sessionId)) {
                                Text("Stop following")
                                    .font(.system(size: 12, weight: .medium))
                            }
                            .buttonStyle(.bordered)
                        }
                    }
                    .padding(.top, 2)
                }
            } compactLeading: {
                FilMonogram(compact: true)
                    .foregroundStyle(context.state.status.color)
            } compactTrailing: {
                Text(timerInterval: context.attributes.startedAt...Date.distantFuture, countsDown: false)
                    .font(.system(size: 12, weight: .semibold, design: .monospaced))
                    .monospacedDigit()
                    .foregroundStyle(context.state.status.color)
            } minimal: {
                FilMonogram(compact: true)
                    .foregroundStyle(context.state.status.color)
            }
            .widgetURL(FilActivityURL.session(context.attributes.sessionId))
            .keylineTint(context.state.status.color)
        }
    }
}

private struct FilLockScreenView: View {
    let context: ActivityViewContext<FilActivityAttributes>
    @Environment(\.isLuminanceReduced) private var isLuminanceReduced

    var body: some View {
        HStack(spacing: 12) {
            FilMonogram()
                .foregroundStyle(context.state.status.color)
                .frame(width: 34, height: 34)
                .background(context.state.status.color.opacity(isLuminanceReduced ? 0.08 : 0.14))
                .clipShape(RoundedRectangle(cornerRadius: 9))

            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(context.attributes.machineName)
                        .font(.system(size: 14, weight: .semibold))
                        .lineLimit(1)
                    Circle()
                        .fill(context.state.status.color)
                        .frame(width: 6, height: 6)
                }
                Text(context.state.projectName)
                    .font(.system(size: 13))
                    .lineLimit(1)
                HStack(spacing: 4) {
                    Text(context.state.status.label)
                    if !context.state.shell.isEmpty { Text("· \(context.state.shell)") }
                    if context.state.otherSessionCount > 0 {
                        Text(
                            "· \(context.state.otherSessionCount) "
                                + String(
                                    localized: context.state.otherSessionCount == 1
                                        ? "other"
                                        : "others"
                                )
                        )
                    }
                }
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            }

            Spacer(minLength: 8)

            Text(timerInterval: context.attributes.startedAt...Date.distantFuture, countsDown: false)
                .font(.system(size: 13, weight: .medium, design: .monospaced))
                .monospacedDigit()
                .foregroundStyle(context.state.status.color)
        }
        .padding(16)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(
            "Fil terminal on \(context.attributes.machineName), \(context.state.projectName), \(context.state.status.label)"
        )
    }
}
