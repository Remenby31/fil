import ActivityKit
import Foundation

public enum FilActivityStatus: String, Codable, Hashable, Sendable {
    case connected
    case reconnecting
    case stale
    case ended
}

public enum FilActivityPrivacy: String, CaseIterable, Codable, Hashable, Sendable {
    case private_
    case standard
    case detailed

    public var title: LocalizedStringResource {
        switch self {
        case .private_: "Private"
        case .standard: "Standard"
        case .detailed: "Detailed"
        }
    }
}

public struct FilActivityAttributes: ActivityAttributes, Sendable {
    public struct ContentState: Codable, Hashable, Sendable {
        public var status: FilActivityStatus
        public var projectName: String
        public var shell: String
        public var otherSessionCount: Int
        public var lastUpdatedAt: Date

        public init(
            status: FilActivityStatus,
            projectName: String,
            shell: String,
            otherSessionCount: Int,
            lastUpdatedAt: Date = Date()
        ) {
            self.status = status
            self.projectName = projectName
            self.shell = shell
            self.otherSessionCount = otherSessionCount
            self.lastUpdatedAt = lastUpdatedAt
        }
    }

    public var sessionId: String
    public var deviceId: String
    public var machineName: String
    public var startedAt: Date

    public init(sessionId: String, deviceId: String, machineName: String, startedAt: Date) {
        self.sessionId = sessionId
        self.deviceId = deviceId
        self.machineName = machineName
        self.startedAt = startedAt
    }
}

public enum FilActivityURL {
    public static func session(_ sessionId: String) -> URL? {
        var components = URLComponents()
        components.scheme = "fil"
        components.host = "session"
        components.path = "/\(sessionId)"
        return components.url
    }
}

public enum FilActivityProjection {
    public static func projectName(from cwd: String) -> String {
        let components = cwd.split(separator: "/", omittingEmptySubsequences: true)
        if components.count == 2 && (components[0] == "Users" || components[0] == "home") {
            return "Home"
        }
        if cwd == "/" {
            return "Root"
        }
        let trimmed = cwd.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        guard let name = trimmed.split(separator: "/").last else { return "Terminal" }
        let value = String(name).replacingOccurrences(of: "~", with: "")
        return value.isEmpty ? "Terminal" : value
    }

    public static func content(
        status: FilActivityStatus,
        cwd: String,
        shell: String,
        otherSessionCount: Int,
        privacy: FilActivityPrivacy
    ) -> FilActivityAttributes.ContentState {
        switch privacy {
        case .private_:
            return .init(
                status: status,
                projectName: String(localized: "Active terminal"),
                shell: "",
                otherSessionCount: otherSessionCount
            )
        case .standard:
            return .init(
                status: status,
                projectName: projectName(from: cwd),
                shell: "",
                otherSessionCount: otherSessionCount
            )
        case .detailed:
            return .init(
                status: status,
                projectName: projectName(from: cwd),
                shell: shell,
                otherSessionCount: otherSessionCount
            )
        }
    }
}
