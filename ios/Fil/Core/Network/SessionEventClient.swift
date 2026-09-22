import Foundation

enum SessionEventClient {
    /// Backoff cap. 30s left the device list stale for half a minute after a
    /// blip; the control plane is one cheap websocket, so retry sooner.
    private static let maxBackoff: TimeInterval = 8
    /// Keeps the socket alive through Cloudflare's ~100s idle cutoff and gives
    /// URLSession a reason to notice a dead peer sooner than its own 60s-3min.
    private static let pingInterval: TimeInterval = 30

    static func updates(
        makeSocket: @escaping @Sendable () -> TerminalWebSocketTask? = {
            guard let token = TokenStorage.loadToken(),
                  let request = SessionEventClient.request(hubURL: TokenStorage.loadHubUrl(), token: token) else { return nil }
            return URLSession.shared.webSocketTask(with: request)
        }
    ) -> AsyncStream<Result<[DeviceState], Error>> {
        AsyncStream { continuation in
            let task = Task {
                var attempt = 0
                while !Task.isCancelled {
                    guard let socket = makeSocket() else {
                        // Do NOT finish the stream. A nil token here usually
                        // means the Keychain is still locked right after boot;
                        // finishing killed the control plane permanently, with
                        // nothing able to restart it.
                        attempt += 1
                        try? await Task.sleep(for: .seconds(backoff(attempt)))
                        continue
                    }

                    socket.resume()

                    let pinger = Task {
                        while !Task.isCancelled {
                            try? await Task.sleep(for: .seconds(pingInterval))
                            guard !Task.isCancelled else { return }
                            socket.sendPing { error in
                                if error != nil { socket.cancel(with: .goingAway, reason: nil) }
                            }
                        }
                    }

                    do {
                        while !Task.isCancelled {
                            let message = try await withTaskCancellationHandler {
                                try Task.checkCancellation()
                                return try await withCheckedThrowingContinuation { continuation in
                                    socket.receive { continuation.resume(with: $0) }
                                }
                            } onCancel: {
                                // Cancelling the consumer does not itself cancel
                                // a callback-based receive. Closing unblocks it.
                                socket.cancel(with: .goingAway, reason: nil)
                            }
                            guard case .string(let text) = message,
                                  let data = text.data(using: .utf8) else { continue }
                            // A malformed frame used to throw out of the read
                            // loop and tear down the socket. Skip it instead.
                            guard let states = try? JSONDecoder().decode(
                                [DeviceState].self, from: data
                            ) else { continue }
                            // Reset on a healthy read, not on a decode success.
                            attempt = 0
                            continuation.yield(.success(states))
                        }
                    } catch {
                        if !Task.isCancelled { continuation.yield(.failure(error)) }
                    }

                    pinger.cancel()
                    socket.cancel(with: .goingAway, reason: nil)
                    guard !Task.isCancelled else { return }
                    attempt += 1
                    try? await Task.sleep(for: .seconds(backoff(attempt)))
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func backoff(_ attempt: Int) -> TimeInterval {
        let raw = pow(2, Double(min(attempt, 10)))
        return min(raw, maxBackoff) * Double.random(in: 0.85...1.15)
    }

    static func request(hubURL: String, token: String) -> URLRequest? {
        guard !token.isEmpty, token.utf8.allSatisfy({ (0x21...0x7e).contains($0) }),
              var components = URLComponents(string: hubURL),
              components.host != nil,
              components.user == nil, components.password == nil else { return nil }
        #if DEBUG && targetEnvironment(simulator)
        let localHTTP = components.scheme?.lowercased() == "http"
            && ["localhost", "127.0.0.1", "::1", "[::1]"].contains(components.host?.lowercased() ?? "")
        #else
        let localHTTP = false
        #endif
        guard components.scheme?.lowercased() == "https" || localHTTP else { return nil }
        components.scheme = localHTTP ? "ws" : "wss"
        components.path = "/ws/client"
        components.query = nil
        components.fragment = nil
        guard let url = components.url else { return nil }
        var request = URLRequest(url: url)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }
}
