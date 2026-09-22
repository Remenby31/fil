import Foundation

actor HubClient {
    private let baseURL: URL
    private let session: URLSession

    init(baseURL: URL? = nil) {
        self.baseURL = baseURL
            ?? URL(string: TokenStorage.loadHubUrl())
            ?? URL(string: "https://fil.remenby.fr")!
        self.session = URLSession.shared
    }

    // MARK: - Auth

    func githubAuthURL() -> URL {
        baseURL.appendingPathComponent("auth/github/start")
    }

    // MARK: - Devices

    func registerDevice(name: String, token: String) async throws -> DeviceResponse {
        let body = RegisterDeviceRequest(name: name, os: "iOS", hostname: nil)
        return try await post("/devices", body: body, token: token)
    }

    func listDevices(token: String) async throws -> [DeviceResponse] {
        try await get("/devices", token: token)
    }

    // MARK: - Sessions

    func listSessions(token: String) async throws -> [DeviceState] {
        try await get("/sessions", token: token)
    }

    /// Mint a single-use, short-lived ticket authorising one QUIC attach.
    ///
    /// The QUIC data plane used to accept any peer that knew a session id. The
    /// account JWT is deliberately not used here: it lives 30 days, so putting
    /// it on the data plane would turn a leaked session id into a leaked
    /// account.
    func sessionTicket(sessionId: String, token: String) async throws -> SessionTicketResponse {
        // The certificate pin is trustworthy only when delivered over HTTPS.
        // Never send the account credential to an HTTP ticket endpoint.
        guard baseURL.scheme?.lowercased() == "https" else { throw HubError.invalidURL }
        let response: SessionTicketResponse = try await post(
            "/sessions/\(sessionId)/ticket",
            body: EmptyBody(),
            token: token
        )
        return response
    }

    // MARK: - Account

    func deleteAccount(token: String) async throws {
        var request = URLRequest(url: baseURL.appendingPathComponent("account"))
        request.httpMethod = "DELETE"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (_, response) = try await session.data(for: request)
        try validateResponse(response)
    }

    // MARK: - Live Activities

    func registerLiveActivity(_ registration: LiveActivityRegistrationRequest, token: String) async throws {
        try await postWithoutResponse("/live-activities", body: registration, token: token)
    }

    func deleteLiveActivity(activityId: String, token: String) async throws {
        var request = URLRequest(url: baseURL.appendingPathComponent("live-activities/\(activityId)"))
        request.httpMethod = "DELETE"
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (_, response) = try await session.data(for: request)
        try validateResponse(response)
    }

    // MARK: - Health

    func health() async throws -> HealthResponse {
        try await get("/health", token: nil)
    }

    // MARK: - HTTP Helpers

    private func get<T: Decodable>(_ path: String, token: String?) async throws -> T {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = "GET"
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        let (data, response) = try await session.data(for: request)
        try validateResponse(response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func post<B: Encodable, T: Decodable>(_ path: String, body: B, token: String?) async throws -> T {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let token {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
        request.httpBody = try JSONEncoder().encode(body)
        let (data, response) = try await session.data(for: request)
        try validateResponse(response)
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func postWithoutResponse<B: Encodable>(_ path: String, body: B, token: String) async throws {
        var request = URLRequest(url: baseURL.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        request.httpBody = try JSONEncoder().encode(body)
        let (_, response) = try await session.data(for: request)
        try validateResponse(response)
    }

    private func validateResponse(_ response: URLResponse) throws {
        guard let http = response as? HTTPURLResponse else {
            throw HubError.invalidResponse
        }
        guard (200...299).contains(http.statusCode) else {
            throw HubError.httpError(http.statusCode)
        }
    }
}

// MARK: - Request/Response Types

struct SessionTicketResponse: Codable, Sendable {
    let ticket: String
    let quicCertificate: String
    enum CodingKeys: String, CodingKey {
        case ticket
        case quicCertificate = "quic_certificate"
    }
}

struct EmptyBody: Codable {}

struct RegisterDeviceRequest: Codable {
    let name: String
    let os: String?
    let hostname: String?
}

struct DeviceResponse: Codable, Equatable, Identifiable {
    let id: String
    let name: String
    let os: String?
    let hostname: String?
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case id, name, os, hostname
        case createdAt = "created_at"
    }
}

struct DeviceState: Codable, Equatable {
    let deviceId: String
    let deviceName: String?
    let userId: String
    let sessions: [SessionDTO]
    let lastHeartbeat: String
    let connected: Bool

    enum CodingKeys: String, CodingKey {
        case deviceId = "device_id"
        case deviceName = "device_name"
        case userId = "user_id"
        case sessions
        case lastHeartbeat = "last_heartbeat"
        case connected
    }
}

struct SessionDTO: Codable, Equatable {
    let sessionId: String
    let deviceId: String
    let shell: String
    let command: String?
    let cwd: String
    let cols: UInt32
    let rows: UInt32
    let status: String
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case sessionId = "session_id"
        case deviceId = "device_id"
        case shell, command, cwd, cols, rows, status
        case createdAt = "created_at"
    }
}

struct LiveActivityRegistrationRequest: Codable {
    let activityId: String
    let sessionId: String
    let deviceId: String
    let pushToken: String
    let environment: String

    enum CodingKeys: String, CodingKey {
        case activityId = "activity_id"
        case sessionId = "session_id"
        case deviceId = "device_id"
        case pushToken = "push_token"
        case environment
    }
}

struct HealthResponse: Codable {
    let status: String
    let version: String
    let service: String
    let liveActivityPushEnabled: Bool?

    enum CodingKeys: String, CodingKey {
        case status, version, service
        case liveActivityPushEnabled = "live_activity_push_enabled"
    }
}

enum HubError: LocalizedError {
    case invalidURL
    case invalidResponse
    case httpError(Int)

    var errorDescription: String? {
        switch self {
        case .invalidURL: "The hub URL is invalid."
        case .invalidResponse: "Invalid server response"
        case .httpError(let code): "Server error (\(code))"
        }
    }
}
