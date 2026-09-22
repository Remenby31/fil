import ActivityKit
import AppIntents
import SwiftUI
import WidgetKit

private extension FilActivityStatus {
    var label: LocalizedStringResource {
        switch self {
        case .connected: "Session available"
        case .reconnecting: "Reconnecting…"
        case .stale: "Update needed"
        case .ended: "Session ended"
        case .stopped: "Following stopped"
        }
    }

    var symbol: String {
        switch self {
        case .connected: "checkmark.circle.fill"
        case .reconnecting: "arrow.triangle.2.circlepath"
        case .stale: "clock.badge.exclamationmark"
        case .ended: "checkmark.circle"
        case .stopped: "pause.circle"
        }
    }

    var color: Color {
        switch self {
        case .connected: .green
        case .reconnecting: .orange
        case .stale, .ended, .stopped: .secondary
        }
    }
}

/// APNs content is never a privacy policy. All visible and accessible text uses
/// this projection, capped by both immutable attributes and local preferences.
private struct FilLiveContent<Content: View>: View {
    let context: ActivityViewContext<FilActivityAttributes>
    @ViewBuilder var content: (FilActivityPresentation) -> Content

    @AppStorage(FilSharedStore.activityPrivacyKey, store: FilSharedStore.activityPrivacyDefaults)
    private var privacyRaw = FilActivityPrivacy.private_.rawValue
    @AppStorage(FilSharedStore.activityPrivacyRevisionKey, store: FilSharedStore.activityPrivacyDefaults)
    private var privacyRevision = "unconfigured"

    init(
        context: ActivityViewContext<FilActivityAttributes>,
        @ViewBuilder content: @escaping (FilActivityPresentation) -> Content
    ) {
        self.context = context
        self.content = content
    }

    var body: some View {
        content(FilActivityProjection.presentation(
            attributes: context.attributes,
            state: context.state,
            isStale: context.isStale,
            privacy: FilActivityPrivacy(rawValue: privacyRaw) ?? .private_,
            privacyRevision: privacyRevision
        ))
    }
}

struct FilLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: FilActivityAttributes.self) { context in
            FilLiveContent(context: context) { display in
                FilLockScreenView(display: display)
            }
            .widgetURL(FilActivityURL.session(context.attributes.sessionId))
            .activityBackgroundTint(Color(uiColor: .secondarySystemBackground))
            .activitySystemActionForegroundColor(.primary)
        } dynamicIsland: { context in
            DynamicIsland {
                DynamicIslandExpandedRegion(.leading) {
                    Text("fil.")
                        .font(.headline.weight(.semibold))
                        .foregroundStyle(.white)
                        .accessibilityLabel("Fil")
                }
                DynamicIslandExpandedRegion(.bottom) {
                    FilLiveContent(context: context) { display in
                        FilExpandedActivityView(display: display, sessionId: context.attributes.sessionId)
                    }
                    .environment(\.colorScheme, .dark)
                }
            } compactLeading: {
                Text("fil.")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.white)
                    .accessibilityLabel("Fil")
            } compactTrailing: {
                FilLiveContent(context: context) { display in
                    Image(systemName: display.status.symbol)
                        .foregroundStyle(display.status.color)
                        .accessibilityLabel(Text(display.status.label))
                }
                .environment(\.colorScheme, .dark)
            } minimal: {
                FilLiveContent(context: context) { display in
                    Image(systemName: display.status.symbol)
                        .foregroundStyle(display.status.color)
                        .accessibilityLabel(Text("Fil") + Text(", ") + Text(display.status.label))
                }
                .environment(\.colorScheme, .dark)
            }
            .widgetURL(FilActivityURL.session(context.attributes.sessionId))
            .keylineTint(.secondary)
        }
    }
}

private struct FilActivityStatusLine: View {
    let display: FilActivityPresentation

    var body: some View {
        Label {
            Text(display.status.label)
        } icon: {
            Image(systemName: display.status.symbol)
                .foregroundStyle(display.status.color)
        }
        .font(.subheadline.weight(.medium))
        .foregroundStyle(.primary)
        .lineLimit(1)
    }
}

private struct FilActivityFreshness: View {
    let display: FilActivityPresentation

    var body: some View {
        if display.status != .ended && display.status != .stopped {
            (Text("Last update") + Text(" · ") + Text(display.lastUpdatedAt, style: .relative))
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
    }
}

private struct FilExpandedActivityView: View {
    let display: FilActivityPresentation
    let sessionId: String
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            if !dynamicTypeSize.isAccessibilitySize {
                Text(display.projectName)
                    .font(.headline)
                    .lineLimit(1)
            }
            FilActivityStatusLine(display: display)
            FilActivityFreshness(display: display)
            Button(intent: StopFollowingFilIntent(sessionId: sessionId)) {
                Label("Stop following", systemImage: "pause.circle")
                    .font(.caption.weight(.semibold))
                    .frame(minHeight: 44)
            }
            .buttonStyle(.bordered)
            .tint(.white)
            .accessibilityHint("Removes the Live Activity without closing the terminal")
        }
        .foregroundStyle(.white)
    }
}

private struct FilLockScreenView: View {
    let display: FilActivityPresentation
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(display.projectName)
                .font(.headline)
                .foregroundStyle(.primary)
                .lineLimit(1)

            if !dynamicTypeSize.isAccessibilitySize, !display.machineName.isEmpty {
                Text(display.shell.isEmpty ? display.machineName : "\(display.machineName) · \(display.shell)")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            FilActivityStatusLine(display: display)
            FilActivityFreshness(display: display)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
        .accessibilityElement(children: .combine)
    }
}
