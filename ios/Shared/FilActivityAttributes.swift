import ActivityKit
import Foundation

public enum FilActivityStatus: String, Codable, Hashable, Sendable {
    case connected
    case reconnecting
    case stale
    case ended
    /// The user stopped displaying the activity, not the remote shell.
    case stopped
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

    public var disclosureLevel: Int {
        switch self {
        case .private_: 0
        case .standard: 1
        case .detailed: 2
        }
    }

    public func limited(to other: Self) -> Self {
        disclosureLevel <= other.disclosureLevel ? self : other
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
    /// Immutable disclosure ceiling. Legacy activities without it fail closed.
    public var privacy: FilActivityPrivacy?
    public var privacyRevision: String?

    public init(
        sessionId: String,
        deviceId: String,
        machineName: String,
        startedAt: Date,
        privacy: FilActivityPrivacy = .private_,
        privacyRevision: String? = nil
    ) {
        self.sessionId = sessionId
        self.deviceId = deviceId
        self.machineName = machineName
        self.startedAt = startedAt
        self.privacy = privacy
        self.privacyRevision = privacyRevision
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
    /// The only metadata views should consume. APNs content is not a privacy policy.
    public static func presentation(
        attributes: FilActivityAttributes,
        state: FilActivityAttributes.ContentState,
        isStale: Bool,
        privacy: FilActivityPrivacy,
        privacyRevision: String
    ) -> FilActivityPresentation {
        let effectivePrivacy: FilActivityPrivacy = attributes.privacyRevision == privacyRevision
            ? (attributes.privacy ?? .private_).limited(to: privacy)
            : .private_
        let status: FilActivityStatus
        switch state.status {
        case .ended, .stopped: status = state.status
        default: status = isStale ? .stale : state.status
        }
        return FilActivityPresentation(
            status: status,
            machineName: effectivePrivacy == .private_ ? "" : String(attributes.machineName.prefix(80)),
            projectName: effectivePrivacy == .private_
                ? String(localized: "Terminal") : String(projectName(from: state.projectName).prefix(80)),
            shell: effectivePrivacy == .detailed ? String(state.shell.prefix(40)) : "",
            lastUpdatedAt: state.lastUpdatedAt
        )
    }

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
                projectName: String(localized: "Terminal"),
                shell: "",
                otherSessionCount: 0
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

public struct FilActivityPresentation: Equatable, Sendable {
    public let status: FilActivityStatus
    public let machineName: String
    public let projectName: String
    public let shell: String
    public let lastUpdatedAt: Date
}
