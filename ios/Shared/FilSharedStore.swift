import Foundation
import WidgetKit

public struct FilWidgetSessionSnapshot: Codable, Hashable, Identifiable, Sendable {
    public let id: String
    public let projectName: String
    public let processName: String
    public let machineName: String

    public init(id: String, projectName: String, processName: String, machineName: String) {
        self.id = id
        self.projectName = projectName
        self.processName = processName
        self.machineName = machineName
    }
}

public struct FilWidgetMachineSnapshot: Codable, Hashable, Identifiable, Sendable {
    public let id: String
    public let name: String
    public let isConnected: Bool
    public let sessions: [FilWidgetSessionSnapshot]

    public init(
        id: String,
        name: String,
        isConnected: Bool,
        sessions: [FilWidgetSessionSnapshot]
    ) {
        self.id = id
        self.name = name
        self.isConnected = isConnected
        self.sessions = sessions
    }
}

public struct FilWidgetSnapshot: Codable, Hashable, Sendable {
    public let updatedAt: Date
    public let machines: [FilWidgetMachineSnapshot]

    public init(updatedAt: Date = Date(), machines: [FilWidgetMachineSnapshot]) {
        self.updatedAt = updatedAt
        self.machines = machines
    }

    public var activeSessions: [FilWidgetSessionSnapshot] {
        machines.flatMap(\.sessions)
    }

    public var connectedMachineCount: Int {
        machines.filter(\.isConnected).count
    }
}

public enum FilSharedStore {
    public static let appGroupID = "group.sh.fil.shared"
    private static let widgetSnapshotKey = "widgetSnapshot"
    private static let terminalFontSizeKey = "terminalFontSize"
    private static let pendingActivityURLKey = "pendingActivityURL"

    private static var defaults: UserDefaults {
        UserDefaults(suiteName: appGroupID) ?? .standard
    }

    public static var terminalFontSize: Double {
        get {
            let stored = defaults.double(forKey: terminalFontSizeKey)
            return stored == 0 ? 14 : min(24, max(10, stored))
        }
        set {
            defaults.set(min(24, max(10, newValue)), forKey: terminalFontSizeKey)
        }
    }

    public static func saveWidgetSnapshot(_ snapshot: FilWidgetSnapshot) {
        guard let data = try? JSONEncoder().encode(snapshot) else { return }
        defaults.set(data, forKey: widgetSnapshotKey)
        WidgetCenter.shared.reloadTimelines(ofKind: "FilWidget")
    }

    public static func loadWidgetSnapshot() -> FilWidgetSnapshot? {
        guard let data = defaults.data(forKey: widgetSnapshotKey) else { return nil }
        return try? JSONDecoder().decode(FilWidgetSnapshot.self, from: data)
    }

    public static func clearWidgetSnapshot() {
        defaults.removeObject(forKey: widgetSnapshotKey)
        WidgetCenter.shared.reloadTimelines(ofKind: "FilWidget")
    }

    public static func savePendingActivityURL(_ url: URL) {
        defaults.set(url.absoluteString, forKey: pendingActivityURLKey)
    }

    public static func takePendingActivityURL() -> URL? {
        guard let value = defaults.string(forKey: pendingActivityURLKey) else { return nil }
        defaults.removeObject(forKey: pendingActivityURLKey)
        return URL(string: value)
    }
}
