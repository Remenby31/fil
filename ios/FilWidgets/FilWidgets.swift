import SwiftUI
import WidgetKit

// MARK: - Timeline Provider

struct FilWidgetProvider: TimelineProvider {
    func placeholder(in context: Context) -> FilWidgetEntry {
        FilWidgetEntry.placeholder
    }

    func getSnapshot(in context: Context, completion: @escaping (FilWidgetEntry) -> Void) {
        completion(.placeholder)
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<FilWidgetEntry>) -> Void) {
        // TODO: Fetch real session data from shared app group
        let entry = FilWidgetEntry(
            date: Date(),
            machines: [
                WidgetMachine(name: "Mac mini", status: .online, sessionCount: 3),
                WidgetMachine(name: "MacBook Pro", status: .offline, sessionCount: 0),
            ]
        )

        let nextUpdate = Calendar.current.date(byAdding: .minute, value: 5, to: Date())!
        let timeline = Timeline(entries: [entry], policy: .after(nextUpdate))
        completion(timeline)
    }
}

// MARK: - Entry

struct FilWidgetEntry: TimelineEntry {
    let date: Date
    let machines: [WidgetMachine]

    var totalSessions: Int {
        machines.reduce(0) { $0 + $1.sessionCount }
    }

    var onlineMachines: Int {
        machines.filter { $0.status == .online }.count
    }

    static let placeholder = FilWidgetEntry(
        date: Date(),
        machines: [
            WidgetMachine(name: "Mac mini", status: .online, sessionCount: 2),
        ]
    )
}

struct WidgetMachine: Identifiable {
    let id = UUID()
    let name: String
    let status: WidgetMachineStatus
    let sessionCount: Int
}

enum WidgetMachineStatus {
    case online, offline
}

// MARK: - Small Widget

struct FilWidgetSmall: View {
    let entry: FilWidgetEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("fil.")
                .font(.system(size: 18, weight: .light))
                .foregroundStyle(.white)

            Spacer()

            HStack(spacing: 4) {
                Circle()
                    .fill(Color(red: 0, green: 0.83, blue: 0.67))
                    .frame(width: 6, height: 6)

                Text("\(entry.totalSessions)")
                    .font(.system(size: 28, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.white)
            }

            Text(entry.totalSessions == 1 ? "session" : "sessions")
                .font(.system(size: 12))
                .foregroundStyle(.white.opacity(0.5))
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .containerBackground(for: .widget) {
            Color(red: 0.039, green: 0.039, blue: 0.059)
        }
    }
}

// MARK: - Medium Widget

struct FilWidgetMedium: View {
    let entry: FilWidgetEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("fil.")
                    .font(.system(size: 16, weight: .light))
                    .foregroundStyle(.white)

                Spacer()

                Text("\(entry.onlineMachines) online")
                    .font(.system(size: 11))
                    .foregroundStyle(.white.opacity(0.4))
            }

            ForEach(entry.machines) { machine in
                HStack(spacing: 6) {
                    Circle()
                        .fill(machine.status == .online
                              ? Color(red: 0, green: 0.83, blue: 0.67)
                              : Color.white.opacity(0.2))
                        .frame(width: 6, height: 6)

                    Text(machine.name)
                        .font(.system(size: 13, weight: .medium))
                        .foregroundStyle(.white)

                    Spacer()

                    if machine.sessionCount > 0 {
                        Text("\(machine.sessionCount) sessions")
                            .font(.system(size: 11, design: .monospaced))
                            .foregroundStyle(.white.opacity(0.4))
                    } else {
                        Text("offline")
                            .font(.system(size: 11))
                            .foregroundStyle(.white.opacity(0.25))
                    }
                }
                .padding(.vertical, 2)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .containerBackground(for: .widget) {
            Color(red: 0.039, green: 0.039, blue: 0.059)
        }
    }
}

// MARK: - Widget Configuration

struct FilWidget: Widget {
    let kind: String = "FilWidget"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: FilWidgetProvider()) { entry in
            FilWidgetEntryView(entry: entry)
        }
        .configurationDisplayName("Fil")
        .description("See your active terminal sessions.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

struct FilWidgetEntryView: View {
    @Environment(\.widgetFamily) var family
    let entry: FilWidgetEntry

    var body: some View {
        switch family {
        case .systemSmall:
            FilWidgetSmall(entry: entry)
        case .systemMedium:
            FilWidgetMedium(entry: entry)
        default:
            FilWidgetSmall(entry: entry)
        }
    }
}

// MARK: - Widget Bundle

@main
struct FilWidgetBundle: WidgetBundle {
    var body: some Widget {
        FilWidget()
        FilLiveActivity()
    }
}
