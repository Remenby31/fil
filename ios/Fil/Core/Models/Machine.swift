import Foundation

struct Machine: Equatable, Identifiable, Codable {
    let id: String
    let name: String
    var status: MachineStatus
    var sessions: [Session]

    var activeSessions: [Session] {
        sessions.filter { $0.status == .online }
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
    var cwd: String
    var cols: UInt32
    var rows: UInt32
    var status: SessionStatus
    let createdAt: Date

    var duration: String {
        let interval = Date().timeIntervalSince(createdAt)
        if interval < 60 { return "\(Int(interval))s" }
        if interval < 3600 { return "\(Int(interval / 60)) min" }
        return "\(Int(interval / 3600))h \(Int((interval.truncatingRemainder(dividingBy: 3600)) / 60))m"
    }
}

enum SessionStatus: String, Equatable, Codable {
    case online
    case unreachable
    case offline
}
