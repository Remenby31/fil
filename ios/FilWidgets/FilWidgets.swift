import SwiftUI
import WidgetKit

struct FilWidgetProvider: TimelineProvider {
    func placeholder(in context: Context) -> FilWidgetEntry {
        .placeholder
    }

    func getSnapshot(in context: Context, completion: @escaping (FilWidgetEntry) -> Void) {
        completion(context.isPreview ? .placeholder : .current)
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<FilWidgetEntry>) -> Void) {
        let entry = FilWidgetEntry.current
        let refresh = Calendar.current.date(byAdding: .minute, value: 15, to: Date())
            ?? Date().addingTimeInterval(900)
        completion(Timeline(entries: [entry], policy: .after(refresh)))
    }
}

struct FilWidgetEntry: TimelineEntry {
    let date: Date
    let snapshot: FilWidgetSnapshot?

    static var current: Self {
        .init(date: Date(), snapshot: FilSharedStore.loadWidgetSnapshot())
    }

    static let placeholder = FilWidgetEntry(
        date: Date(),
        snapshot: FilWidgetSnapshot(
            machines: [
                FilWidgetMachineSnapshot(
                    id: "preview-machine",
                    name: "MacBook Pro",
                    isConnected: true,
                    sessions: [
                        FilWidgetSessionSnapshot(
                            id: "preview-session",
                            projectName: "cerebro-map",
                            processName: "Codex",
                            machineName: "MacBook Pro"
                        )
                    ]
                )
            ]
        )
    )
}

private enum FilWidgetColors {
    static let background = Color(uiColor: .secondarySystemBackground)
}

private struct FilWidgetMark: View {
    var body: some View {
        HStack(spacing: 0) {
            Text("fil")
                .foregroundStyle(.primary)
            Text(".sh")
                .foregroundStyle(.primary)
        }
        .font(.headline.weight(.medium))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Fil")
    }
}

struct FilWidgetSmall: View {
    let entry: FilWidgetEntry
    let privacy: FilActivityPrivacy
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            FilWidgetMark()

            Spacer()

            if let snapshot = entry.snapshot {
                Text("\(snapshot.activeSessions.count)")
                    .font(.system(.largeTitle, design: .rounded, weight: .semibold))
                    .monospacedDigit()

                Text("Terminals at last sync")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                if privacy != .private_, !dynamicTypeSize.isAccessibilitySize,
                   let session = snapshot.activeSessions.first {
                    Text(session.projectName)
                        .font(.caption.weight(.medium))
                        .lineLimit(1)
                }
                FilWidgetSyncTime(date: snapshot.updatedAt)
            } else {
                Text("Open Fil")
                    .font(.headline)
                Text("to sync your terminals")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .containerBackground(for: .widget) {
            FilWidgetColors.background
        }
    }
}

struct FilWidgetMedium: View {
    let entry: FilWidgetEntry
    let privacy: FilActivityPrivacy
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                FilWidgetMark()
                Spacer()
            }

            if let snapshot = entry.snapshot, !snapshot.activeSessions.isEmpty {
                if privacy == .private_ {
                    Text("\(snapshot.activeSessions.count) terminals at last sync")
                        .font(.headline)
                } else {
                    ForEach(Array(snapshot.activeSessions.prefix(dynamicTypeSize.isAccessibilitySize ? 1 : 2))) { session in
                        HStack(spacing: 8) {
                            Image(systemName: "terminal")
                                .foregroundStyle(.secondary)
                                .accessibilityHidden(true)

                            VStack(alignment: .leading, spacing: 1) {
                                Text(session.projectName)
                                    .font(.caption.weight(.semibold))
                                    .lineLimit(1)
                                Text(privacy == .detailed ? "\(session.processName) · \(session.machineName)" : session.machineName)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                                    .lineLimit(1)
                            }

                            Spacer(minLength: 4)
                        }
                        .accessibilityElement(children: .combine)
                    }
                }
            } else {
                Text(entry.snapshot == nil
                     ? LocalizedStringKey("Open Fil to refresh")
                     : LocalizedStringKey("No terminals at last sync"))
                    .font(.subheadline)
            }
            if let snapshot = entry.snapshot {
                FilWidgetSyncTime(date: snapshot.updatedAt)
            }

            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .containerBackground(for: .widget) {
            FilWidgetColors.background
        }
    }
}

struct FilWidget: Widget {
    let kind = "FilWidget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: FilWidgetProvider()) { entry in
            FilWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Fil Terminals")
        .description("See your last synced terminals. Open Fil for current status.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

struct FilWidgetEntryView: View {
    @Environment(\.widgetFamily) private var family
    let entry: FilWidgetEntry
    @AppStorage(FilSharedStore.activityPrivacyKey, store: FilSharedStore.activityPrivacyDefaults)
    private var privacyRaw = FilActivityPrivacy.private_.rawValue

    private var privacy: FilActivityPrivacy {
        FilActivityPrivacy(rawValue: privacyRaw) ?? .private_
    }

    var body: some View {
        switch family {
        case .systemMedium:
            FilWidgetMedium(entry: entry, privacy: privacy)
        default:
            FilWidgetSmall(entry: entry, privacy: privacy)
        }
    }
}

private struct FilWidgetSyncTime: View {
    let date: Date

    var body: some View {
        (Text("Last sync") + Text(" · ") + Text(date, style: .relative))
            .font(.caption)
            .foregroundStyle(.secondary)
            .lineLimit(1)
    }
}

@main
struct FilWidgetBundle: WidgetBundle {
    var body: some Widget {
        FilWidget()
        FilLiveActivity()
    }
}
