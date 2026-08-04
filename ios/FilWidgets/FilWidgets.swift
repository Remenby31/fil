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
    static let accent = Color(red: 0, green: 0.83, blue: 0.67)
    static let background = Color(red: 0.039, green: 0.039, blue: 0.059)
}

private struct FilWidgetMark: View {
    var body: some View {
        HStack(spacing: 0) {
            Text("fil")
                .foregroundStyle(.primary)
            Text(".sh")
                .foregroundStyle(FilWidgetColors.accent)
        }
        .font(.headline.weight(.medium))
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Fil")
    }
}

struct FilWidgetSmall: View {
    let entry: FilWidgetEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            FilWidgetMark()

            Spacer()

            if let snapshot = entry.snapshot {
                Text("\(snapshot.activeSessions.count)")
                    .font(.system(.largeTitle, design: .rounded, weight: .semibold))
                    .monospacedDigit()

                Text(
                    snapshot.activeSessions.count == 1
                        ? String(localized: "active terminal")
                        : String(localized: "active terminals")
                )
                    .font(.caption)
                    .foregroundStyle(.secondary)

                if let session = snapshot.activeSessions.first {
                    Text(session.projectName)
                        .font(.caption.weight(.medium))
                        .lineLimit(1)
                }
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

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                FilWidgetMark()
                Spacer()
                if let snapshot = entry.snapshot {
                    Text("\(snapshot.connectedMachineCount) connected")
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                }
            }

            if let snapshot = entry.snapshot, !snapshot.activeSessions.isEmpty {
                ForEach(Array(snapshot.activeSessions.prefix(3))) { session in
                    HStack(spacing: 8) {
                        Circle()
                            .fill(FilWidgetColors.accent)
                            .frame(width: 6, height: 6)

                        VStack(alignment: .leading, spacing: 1) {
                            Text(session.projectName)
                                .font(.caption.weight(.semibold))
                                .lineLimit(1)
                            Text("\(session.processName) · \(session.machineName)")
                                .font(.caption2)
                                .foregroundStyle(.secondary)
                                .lineLimit(1)
                        }

                        Spacer(minLength: 4)
                    }
                }
            } else {
                ContentUnavailableView(
                    "No Active Terminals",
                    systemImage: "terminal",
                    description: Text("Open Fil to refresh")
                )
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
        .description("See the terminal sessions that are active right now.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

struct FilWidgetEntryView: View {
    @Environment(\.widgetFamily) private var family
    let entry: FilWidgetEntry

    var body: some View {
        switch family {
        case .systemMedium:
            FilWidgetMedium(entry: entry)
        default:
            FilWidgetSmall(entry: entry)
        }
    }
}

@main
struct FilWidgetBundle: WidgetBundle {
    var body: some Widget {
        FilWidget()
        FilLiveActivity()
    }
}
