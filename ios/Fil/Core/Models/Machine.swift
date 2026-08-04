import Foundation

struct Machine: Equatable, Identifiable, Codable {
    let id: String
    let name: String
    var status: MachineStatus
    var sessions: [Session]

    var activeSessions: [Session] {
        sessions.filter { $0.status == .online }
    }

    var displayName: String {
        name.replacingOccurrences(of: "-", with: " ")
    }
}

enum MachineStatus: String, Equatable, Codable {
    case online
    case unreachable
    case offline
}

struct Session: Equatable, Identifiable, Codable {
    let id: String
    let deviceId: String
    let shell: String
    let command: String?
    var cwd: String
    var cols: UInt32
    var rows: UInt32
    var status: SessionStatus
    let createdAt: Date?

    var shellName: String {
        shell.split(separator: "/").last.map(String.init) ?? shell
    }

    private var executableName: String {
        let candidate = command?.trimmingCharacters(in: .whitespacesAndNewlines)
        let raw = candidate?.isEmpty == false ? candidate ?? shellName : shellName
        let executable = raw.split(whereSeparator: { $0.isWhitespace }).first.map(String.init) ?? raw
        return executable.split(separator: "/").last.map(String.init) ?? executable
    }

    var processName: String {
        switch executableName.lowercased() {
        case let name where name == "codex" || name.hasPrefix("codex-"): return "Codex"
        case let name where name == "claude" || name.hasPrefix("claude-"): return "Claude Code"
        case "node": return "Node.js"
        case let name where name.hasPrefix("python"): return "Python"
        case "zsh", "bash", "fish", "sh", "dash": return "Shell"
        default: return executableName
        }
    }

    var isShellOnly: Bool {
        ["zsh", "bash", "fish", "sh", "dash"].contains(executableName.lowercased())
    }

    var compactPath: String {
        let components = cwd.split(separator: "/", omittingEmptySubsequences: true)
        guard components.count >= 2 else { return cwd.isEmpty ? "—" : cwd }
        let isUserHome = (components[0] == "Users" || components[0] == "home")
        guard isUserHome else { return cwd }
        let relative = components.dropFirst(2).joined(separator: "/")
        return relative.isEmpty ? "~" : "~/\(relative)"
    }

    var projectName: String {
        guard !cwd.isEmpty else { return processName }
        guard compactPath != "~" else { return "Home" }
        guard compactPath != "/" else { return "Root" }
        guard let name = cwd.split(separator: "/", omittingEmptySubsequences: true).last else {
            return processName
        }
        return String(name)
    }

    var detail: String {
        projectName.compare(processName, options: [.caseInsensitive, .diacriticInsensitive]) == .orderedSame
            ? compactPath
            : "\(processName) · \(compactPath)"
    }

    var parentCompactPath: String {
        guard compactPath != "~", compactPath != "/", compactPath != "—" else {
            return compactPath
        }
        let components = compactPath.split(separator: "/", omittingEmptySubsequences: true)
        guard components.count > 1 else { return compactPath }
        let parent = components.dropLast().joined(separator: "/")
        return compactPath.hasPrefix("/") ? "/\(parent)" : parent
    }

    var listDetail: String {
        "\(processName) · \(parentCompactPath)"
    }

    func duration(at date: Date = Date()) -> String? {
        guard let createdAt,
              createdAt.timeIntervalSince1970 > 86_400,
              createdAt <= date.addingTimeInterval(300) else {
            return nil
        }
        let interval = max(0, date.timeIntervalSince(createdAt))
        if interval < 60 { return "<1 min" }
        if interval < 3600 { return "\(Int(interval / 60)) min" }
        if interval < 86_400 {
            let hours = Int(interval / 3600)
            let minutes = Int((interval.truncatingRemainder(dividingBy: 3600)) / 60)
            return minutes == 0 ? "\(hours)h" : "\(hours)h \(minutes)m"
        }
        let days = Int(interval / 86_400)
        let hours = Int((interval.truncatingRemainder(dividingBy: 86_400)) / 3600)
        return hours == 0 ? "\(days)d" : "\(days)d \(hours)h"
    }

    func accessibilityDescription(at date: Date = Date()) -> String {
        var parts = [projectName, detail]
        if let duration = duration(at: date) {
            parts.append("active for \(duration)")
        }
        return parts.joined(separator: ", ")
    }
}

enum SessionStatus: String, Equatable, Codable {
    case online
    case unreachable
    case offline
}
