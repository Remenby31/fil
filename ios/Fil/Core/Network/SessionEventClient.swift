import Foundation

enum SessionEventClient {
    static func updates() -> AsyncStream<Result<[DeviceState], Error>> {
        AsyncStream { continuation in
            let task = Task {
                var attempt = 0
                while !Task.isCancelled {
                    guard let token = TokenStorage.loadToken(),
                          let url = Self.url(token: token) else {
                        continuation.finish()
                        return
                    }
                    let socket = URLSession.shared.webSocketTask(with: url)
                    socket.resume()
                    do {
                        while !Task.isCancelled {
                            let message = try await socket.receive()
                            guard case .string(let text) = message,
                                  let data = text.data(using: .utf8) else { continue }
                            let states = try JSONDecoder().decode([DeviceState].self, from: data)
                            attempt = 0
                            continuation.yield(.success(states))
                        }
                    } catch {
                        continuation.yield(.failure(error))
                    }
                    socket.cancel(with: .goingAway, reason: nil)
                    attempt += 1
                    let delay = min(pow(2, Double(attempt)), 30)
                    try? await Task.sleep(for: .seconds(delay))
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    private static func url(token: String) -> URL? {
        guard var components = URLComponents(string: TokenStorage.loadHubUrl()) else { return nil }
        components.scheme = components.scheme == "https" ? "wss" : "ws"
        components.path = "/ws/client"
        components.queryItems = [URLQueryItem(name: "token", value: token)]
        return components.url
    }
}
