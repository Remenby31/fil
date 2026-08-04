import ComposableArchitecture
import Foundation

struct TerminalSessionContext: Equatable, Identifiable {
    let session: Session
    let machineName: String

    var id: String { session.id }
}

@Reducer
struct TerminalFeature {
    @ObservableState
    struct State: Equatable {
        var session: Session
        var isConnected = false
        var fontSize: CGFloat
        var latencyMs: Int?
        var showDisconnectedAlert = false
        var isFollowing = false
        var isFollowRequestInFlight = false
        var showLiveActivityUnavailableAlert = false
        var machineName: String
        var otherSessionCount: Int
        var availableSessions: [TerminalSessionContext]

        init(
            session: Session,
            machineName: String = "Machine",
            otherSessionCount: Int = 0,
            availableSessions: [TerminalSessionContext] = []
        ) {
            self.session = session
            self.fontSize = CGFloat(FilSharedStore.terminalFontSize)
            self.machineName = machineName
            self.otherSessionCount = otherSessionCount
            self.availableSessions = availableSessions.isEmpty
                ? [.init(session: session, machineName: machineName)]
                : availableSessions
        }
    }

    enum Action: Equatable {
        case onAppear
        case onDisappear
        case connected
        case disconnected
        case inputSent(Data)
        case dismiss
        case fontSizeChanged(CGFloat)
        case terminalSizeChanged(cols: Int, rows: Int)
        case latencyUpdated(Int)
        case reconnectTapped
        case followTapped
        case followingStatusLoaded(Bool)
        case followingChanged(isFollowing: Bool, requestedFollow: Bool)
        case dismissLiveActivityUnavailableAlert
        case nextSession
        case previousSession
        case switchSession(String)
    }

    @Dependency(\.terminalClient) private var terminalClient

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear:
                let session = state.session
                let sessionId = session.id
                let hubHost = Self.quicHost()

                return .merge(
                    .run { send in
                        if #available(iOS 16.2, *) {
                            await send(
                                .followingStatusLoaded(
                                    await FilActivityManager.shared.isFollowing(sessionId: sessionId)
                                )
                            )
                        }
                    },
                    .run { send in
                        for await event in terminalClient.connect(sessionId, hubHost) {
                            switch event {
                            case .connected:
                                await send(.connected)
                            case .disconnected:
                                await send(.disconnected)
                            }
                        }
                    }
                    .cancellable(id: CancelID.quic, cancelInFlight: true)
                )

            case .onDisappear:
                let sessionId = state.session.id
                return .merge(
                    .cancel(id: CancelID.quic),
                    .run { _ in terminalClient.disconnect(sessionId) }
                )

            case .connected:
                state.isConnected = true
                state.showDisconnectedAlert = false
                let session = state.session
                let sessionId = session.id
                let otherSessionCount = state.otherSessionCount
                return .run { _ in
                    if #available(iOS 16.2, *) {
                        await FilActivityManager.shared.update(
                            sessionId: sessionId,
                            status: .connected,
                            cwd: session.cwd,
                            processName: session.processName,
                            otherSessionCount: otherSessionCount
                        )
                    }
                }

            case .disconnected:
                state.isConnected = false
                state.showDisconnectedAlert = true
                let session = state.session
                let otherSessionCount = state.otherSessionCount
                return .run { _ in
                    if #available(iOS 16.2, *) {
                        await FilActivityManager.shared.update(
                            sessionId: session.id,
                            status: .reconnecting,
                            cwd: session.cwd,
                            processName: session.processName,
                            otherSessionCount: otherSessionCount
                        )
                    }
                }

            case .inputSent(let data):
                let sessionId = state.session.id
                return .run { _ in terminalClient.sendInput(sessionId, data) }

            case .dismiss:
                return .none

            case .fontSizeChanged(let size):
                state.fontSize = max(10, min(24, size))
                FilSharedStore.terminalFontSize = Double(state.fontSize)
                return .none

            case .terminalSizeChanged(let cols, let rows):
                let cols = max(1, min(Int(UInt16.max), cols))
                let rows = max(1, min(Int(UInt16.max), rows))
                guard state.session.cols != UInt32(cols) || state.session.rows != UInt32(rows) else {
                    return .none
                }
                state.session.cols = UInt32(cols)
                state.session.rows = UInt32(rows)
                let sessionId = state.session.id
                return .run { _ in
                    terminalClient.resize(sessionId, UInt16(cols), UInt16(rows))
                }

            case .latencyUpdated(let ms):
                state.latencyMs = ms
                return .none

            case .reconnectTapped:
                state.showDisconnectedAlert = false
                return .send(.onAppear)

            case .followTapped:
                guard !state.isFollowRequestInFlight else { return .none }
                state.isFollowRequestInFlight = true

                let shouldFollow = !state.isFollowing
                let session = state.session
                let machineName = state.machineName
                let otherSessionCount = state.otherSessionCount
                let status: FilActivityStatus = state.isConnected ? .connected : .reconnecting

                return .run { send in
                    if #available(iOS 16.2, *) {
                        if shouldFollow {
                            let didFollow = await FilActivityManager.shared.follow(
                                session: session,
                                machineName: machineName,
                                otherSessionCount: otherSessionCount,
                                status: status
                            )
                            await send(
                                .followingChanged(
                                    isFollowing: didFollow,
                                    requestedFollow: true
                                )
                            )
                        } else {
                            await FilActivityManager.shared.stopFollowing(sessionId: session.id)
                            await send(
                                .followingChanged(
                                    isFollowing: false,
                                    requestedFollow: false
                                )
                            )
                        }
                    } else {
                        await send(
                            .followingChanged(
                                isFollowing: false,
                                requestedFollow: shouldFollow
                            )
                        )
                    }
                }

            case .followingStatusLoaded(let isFollowing):
                state.isFollowing = isFollowing
                return .none

            case let .followingChanged(isFollowing, requestedFollow):
                state.isFollowRequestInFlight = false
                state.isFollowing = isFollowing
                state.showLiveActivityUnavailableAlert = requestedFollow && !isFollowing
                return .none

            case .dismissLiveActivityUnavailableAlert:
                state.showLiveActivityUnavailableAlert = false
                return .none

            case .nextSession:
                return switchSession(&state, offset: 1)

            case .previousSession:
                return switchSession(&state, offset: -1)

            case .switchSession(let sessionId):
                guard let index = state.availableSessions.firstIndex(where: { $0.id == sessionId }) else {
                    return .none
                }
                return switchSession(&state, to: index)
            }
        }
    }

    private enum CancelID { case quic }

    private func switchSession(_ state: inout State, offset: Int) -> Effect<Action> {
        guard state.availableSessions.count > 1,
              let currentIndex = state.availableSessions.firstIndex(where: {
                  $0.id == state.session.id
              }) else {
            return .none
        }
        let count = state.availableSessions.count
        let targetIndex = (currentIndex + offset + count) % count
        return switchSession(&state, to: targetIndex)
    }

    private func switchSession(_ state: inout State, to index: Int) -> Effect<Action> {
        let target = state.availableSessions[index]
        guard target.id != state.session.id else { return .none }

        let previousSessionId = state.session.id
        state.session = target.session
        state.machineName = target.machineName
        state.otherSessionCount = max(0, state.availableSessions.count - 1)
        state.isConnected = false
        state.isFollowing = false
        state.showDisconnectedAlert = false
        state.latencyMs = nil

        return .merge(
            .cancel(id: CancelID.quic),
            .run { _ in terminalClient.disconnect(previousSessionId) },
            .send(.onAppear)
        )
    }

    private static func quicHost() -> String {
        let httpHost = TokenStorage.loadHubUrl()
            .replacingOccurrences(of: "https://", with: "")
            .replacingOccurrences(of: "http://", with: "")
            .components(separatedBy: ":").first ?? "localhost"
        if httpHost == "localhost" || httpHost == "127.0.0.1" {
            return httpHost
        }
        return "quic.\(httpHost)"
    }
}

enum TerminalClientEvent {
    case connected
    case disconnected
}

fileprivate struct TerminalClientDependency: Sendable {
    var connect: @Sendable (_ sessionId: String, _ hubHost: String) -> AsyncStream<TerminalClientEvent>
    var disconnect: @Sendable (_ sessionId: String) -> Void
    var sendInput: @Sendable (_ sessionId: String, _ data: Data) -> Void
    var resize: @Sendable (_ sessionId: String, _ cols: UInt16, _ rows: UInt16) -> Void
}

extension TerminalClientDependency: DependencyKey {
    static let liveValue = TerminalClientDependency(
        connect: { sessionId, hubHost in
            TerminalConnectionRegistry.shared.connect(sessionId: sessionId, hubHost: hubHost)
        },
        disconnect: { sessionId in
            TerminalConnectionRegistry.shared.disconnect(sessionId: sessionId)
        },
        sendInput: { sessionId, data in
            TerminalConnectionRegistry.shared.sendInput(sessionId: sessionId, data: data)
        },
        resize: { sessionId, cols, rows in
            TerminalConnectionRegistry.shared.resize(sessionId: sessionId, cols: cols, rows: rows)
        }
    )

    static let testValue = TerminalClientDependency(
        connect: { _, _ in AsyncStream { $0.finish() } },
        disconnect: { _ in },
        sendInput: { _, _ in },
        resize: { _, _, _ in }
    )
}

final class TerminalConnectionRegistry: @unchecked Sendable {
    static let shared = TerminalConnectionRegistry()

    private let lock = NSLock()
    private var clients: [String: QUICTerminalClient] = [:]
    private var outputRelays: [String: TerminalOutputRelay] = [:]

    func outputRelay(sessionId: String) -> TerminalOutputRelay {
        lock.filWithLock {
            if let relay = outputRelays[sessionId] {
                return relay
            }
            let relay = TerminalOutputRelay()
            outputRelays[sessionId] = relay
            return relay
        }
    }

    func connect(sessionId: String, hubHost: String) -> AsyncStream<TerminalClientEvent> {
        AsyncStream { continuation in
            let client = QUICTerminalClient(hubHost: hubHost)
            let outputRelay = outputRelay(sessionId: sessionId)
            client.onConnected = {
                continuation.yield(.connected)
            }
            client.onDisconnected = {
                continuation.yield(.disconnected)
            }
            client.onDataReceived = { data in
                outputRelay.enqueue(data)
            }

            lock.filWithLock {
                clients[sessionId]?.disconnect()
                clients[sessionId] = client
            }

            continuation.onTermination = { [weak self, weak client] _ in
                client?.disconnect()
                self?.remove(sessionId: sessionId, client: client)
            }

            client.connect(sessionId: sessionId)
        }
    }

    func disconnect(sessionId: String) {
        let client = lock.filWithLock {
            outputRelays.removeValue(forKey: sessionId)
            return clients.removeValue(forKey: sessionId)
        }
        client?.disconnect()
    }

    func sendInput(sessionId: String, data: Data) {
        let client = lock.filWithLock { clients[sessionId] }
        client?.sendInput(data)
    }

    func resize(sessionId: String, cols: UInt16, rows: UInt16) {
        let client = lock.filWithLock { clients[sessionId] }
        client?.sendResize(cols: cols, rows: rows)
    }

    private func remove(sessionId: String, client: QUICTerminalClient?) {
        lock.filWithLock {
            if let client, clients[sessionId] === client {
                clients.removeValue(forKey: sessionId)
                outputRelays.removeValue(forKey: sessionId)
            }
        }
    }
}

extension DependencyValues {
    fileprivate var terminalClient: TerminalClientDependency {
        get { self[TerminalClientDependency.self] }
        set { self[TerminalClientDependency.self] = newValue }
    }
}

extension NSLock {
    func filWithLock<T>(_ body: () throws -> T) rethrows -> T {
        lock()
        defer { unlock() }
        return try body()
    }
}
