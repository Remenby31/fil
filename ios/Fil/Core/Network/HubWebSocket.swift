import Foundation

@Observable
final class HubWebSocket: @unchecked Sendable {
    private var webSocket: URLSessionWebSocketTask?
    private let session = URLSession(configuration: .default)
    private(set) var isConnected = false
    private var reconnectTask: Task<Void, Never>?

    var onDeviceStateUpdate: (([DeviceState]) -> Void)?

    func connect(hubURL: String, token: String) {
        guard var components = URLComponents(string: hubURL) else { return }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        components.path = "/ws/client"
        components.queryItems = [URLQueryItem(name: "token", value: token)]

        guard let url = components.url else { return }

        let ws = session.webSocketTask(with: url)
        self.webSocket = ws
        ws.resume()
        isConnected = true

        receiveLoop()
    }

    func disconnect() {
        reconnectTask?.cancel()
        reconnectTask = nil
        webSocket?.cancel(with: .goingAway, reason: nil)
        webSocket = nil
        isConnected = false
    }

    private func receiveLoop() {
        webSocket?.receive { [weak self] result in
            switch result {
            case .success(let message):
                if case .string(let text) = message {
                    self?.handleMessage(text)
                }
                self?.receiveLoop()

            case .failure:
                self?.isConnected = false
                self?.scheduleReconnect()
            }
        }
    }

    private func handleMessage(_ text: String) {
        guard let data = text.data(using: .utf8),
              let states = try? JSONDecoder().decode([DeviceState].self, from: data) else { return }
        Task { @MainActor in
            onDeviceStateUpdate?(states)
        }
    }

    private func scheduleReconnect() {
        reconnectTask = Task {
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            // TODO: reconnect with stored credentials
        }
    }
}
