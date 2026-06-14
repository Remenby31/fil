import Foundation
import Network
import Combine

@Observable
final class ConnectionManager: @unchecked Sendable {
    private(set) var isConnected = false
    private(set) var isMonitoringNetwork = false
    private var networkMonitor: NWPathMonitor?
    private var monitorQueue = DispatchQueue(label: "sh.fil.network")
    private var webSocket: URLSessionWebSocketTask?
    private var reconnectAttempt = 0
    private let maxReconnectDelay: TimeInterval = 30

    var onSessionsUpdated: (([DeviceState]) -> Void)?

    func startMonitoring() {
        guard !isMonitoringNetwork else { return }
        isMonitoringNetwork = true

        let monitor = NWPathMonitor()
        monitor.pathUpdateHandler = { [weak self] path in
            Task { @MainActor in
                if path.status == .satisfied {
                    self?.connect()
                } else {
                    self?.disconnect()
                }
            }
        }
        monitor.start(queue: monitorQueue)
        networkMonitor = monitor
    }

    func stopMonitoring() {
        networkMonitor?.cancel()
        networkMonitor = nil
        isMonitoringNetwork = false
        disconnect()
    }

    func connect() {
        guard let token = TokenStorage.loadToken() else { return }
        let hubURL = TokenStorage.loadHubUrl()

        guard var components = URLComponents(string: hubURL) else { return }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        components.path = "/ws/client"
        components.queryItems = [URLQueryItem(name: "token", value: token)]

        guard let url = components.url else { return }

        let session = URLSession(configuration: .default)
        let ws = session.webSocketTask(with: url)
        self.webSocket = ws
        ws.resume()

        Task { @MainActor in
            isConnected = true
            reconnectAttempt = 0
        }

        receiveLoop(ws)
    }

    func disconnect() {
        webSocket?.cancel(with: .goingAway, reason: nil)
        webSocket = nil
        Task { @MainActor in
            isConnected = false
        }
    }

    private func receiveLoop(_ ws: URLSessionWebSocketTask) {
        ws.receive { [weak self] result in
            switch result {
            case .success(let message):
                if case .string(let text) = message {
                    self?.handleMessage(text)
                }
                self?.receiveLoop(ws)

            case .failure:
                Task { @MainActor in
                    self?.isConnected = false
                    self?.scheduleReconnect()
                }
            }
        }
    }

    private func handleMessage(_ text: String) {
        guard let data = text.data(using: .utf8),
              let states = try? JSONDecoder().decode([DeviceState].self, from: data) else { return }
        Task { @MainActor in
            onSessionsUpdated?(states)
        }
    }

    private func scheduleReconnect() {
        reconnectAttempt += 1
        let delay = min(pow(2.0, Double(reconnectAttempt)), maxReconnectDelay)

        Task {
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            await MainActor.run {
                if !isConnected {
                    connect()
                }
            }
        }
    }
}
