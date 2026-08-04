import ComposableArchitecture
import Foundation
import Network

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
        var connectionState: TerminalConnectionState = .connecting
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
        case connectionStateChanged(TerminalConnectionState)
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
                        for await connectionState in terminalClient.open(sessionId, hubHost) {
                            await send(.connectionStateChanged(connectionState))
                        }
                    }
                    .cancellable(id: CancelID.quic, cancelInFlight: true)
                )

            case .onDisappear:
                let sessionId = state.session.id
                return .merge(
                    .cancel(id: CancelID.quic),
                    .run { _ in terminalClient.close(sessionId) }
                )

            case .connectionStateChanged(let connectionState):
                state.connectionState = connectionState
                state.isConnected = connectionState == .connected
                // Only the terminal states warrant a modal; a retry in flight
                // is shown inline so a two-second blip does not black out the
                // screen.
                state.showDisconnectedAlert = connectionState.needsUserAction
                let session = state.session
                let otherSessionCount = state.otherSessionCount
                let activityStatus = connectionState.activityStatus
                return .run { _ in
                    if #available(iOS 16.2, *) {
                        await FilActivityManager.shared.update(
                            sessionId: session.id,
                            status: activityStatus,
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
                // No early-return on an unchanged size. The reducer's copy
                // survives reconnects while the connection's does not, so
                // skipping here is exactly what left a reconnected PTY stuck
                // at the Mac's geometry. Per-connection dedup lives in
                // QUICTerminalClient, which resets it on every connect.
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
                // Do NOT re-send .onAppear: the subscription is already live
                // and restarting it would only churn the effect. Just ask the
                // session for a fresh connection.
                state.showDisconnectedAlert = false
                let sessionId = state.session.id
                return .run { _ in terminalClient.reconnect(sessionId) }

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
        state.connectionState = .connecting
        state.isFollowing = false
        state.showDisconnectedAlert = false
        state.latencyMs = nil

        return .merge(
            .cancel(id: CancelID.quic),
            .run { _ in terminalClient.close(previousSessionId) },
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

fileprivate struct TerminalClientDependency: Sendable {
    var open: @Sendable (_ sessionId: String, _ hubHost: String) -> AsyncStream<TerminalConnectionState>
    var close: @Sendable (_ sessionId: String) -> Void
    var reconnect: @Sendable (_ sessionId: String) -> Void
    var sendInput: @Sendable (_ sessionId: String, _ data: Data) -> Void
    var resize: @Sendable (_ sessionId: String, _ cols: UInt16, _ rows: UInt16) -> Void
}

extension TerminalClientDependency: DependencyKey {
    static let liveValue = TerminalClientDependency(
        open: { sessionId, hubHost in
            TerminalConnectionRegistry.shared.open(sessionId: sessionId, hubHost: hubHost)
        },
        close: { sessionId in
            TerminalConnectionRegistry.shared.close(sessionId: sessionId)
        },
        reconnect: { sessionId in
            TerminalConnectionRegistry.shared.reconnect(sessionId: sessionId)
        },
        sendInput: { sessionId, data in
            TerminalConnectionRegistry.shared.sendInput(sessionId: sessionId, data: data)
        },
        resize: { sessionId, cols, rows in
            TerminalConnectionRegistry.shared.resize(sessionId: sessionId, cols: cols, rows: rows)
        }
    )

    static let testValue = TerminalClientDependency(
        open: { _, _ in AsyncStream { $0.finish() } },
        close: { _ in },
        reconnect: { _ in },
        sendInput: { _, _ in },
        resize: { _, _, _ in }
    )
}

/// Owns `TerminalSession`s. Sessions are created on first use and destroyed
/// only by an explicit `close` — never by a view disappearing or a TCA effect
/// being cancelled.
final class TerminalConnectionRegistry: @unchecked Sendable {
    static let shared = TerminalConnectionRegistry()

    private let lock = NSLock()
    private var sessions: [String: TerminalSession] = [:]
    private let pathMonitor = NWPathMonitor()
    private var isMonitoringPath = false

    /// Overridable so tests can drive the state machine without networking.
    var makeTransport: @Sendable (_ sessionId: String, _ hubHost: String) -> TerminalTransport = {
        _, hubHost in
        QUICTerminalClient(hubHost: hubHost)
    }

    /// The relay for a session, created with the session and stable for its
    /// whole life. `makeUIView` can run before `open`, so this creates the
    /// session record on demand.
    func outputRelay(sessionId: String) -> TerminalOutputRelay {
        session(sessionId: sessionId, hubHost: nil).outputRelay
    }

    private func session(sessionId: String, hubHost: String?) -> TerminalSession {
        lock.filWithLock {
            if let existing = sessions[sessionId] {
                return existing
            }
            let host = hubHost ?? ""
            let make = makeTransport
            let created = TerminalSession(sessionId: sessionId) { sid in
                make(sid, host)
            }
            sessions[sessionId] = created
            return created
        }
    }

    /// Subscribe to a session's state, connecting it if it is not already up.
    /// Idempotent: repeated calls attach another subscriber to the same live
    /// connection instead of tearing it down and rebuilding it.
    func open(sessionId: String, hubHost: String) -> AsyncStream<TerminalConnectionState> {
        startPathMonitoringIfNeeded()
        let session = session(sessionId: sessionId, hubHost: hubHost)
        let stream = session.subscribe()
        session.connectIfNeeded()
        return stream
    }

    /// One monitor for the whole app. `pathUpdateHandler` fires on every path
    /// change, including redundant `.satisfied` callbacks, so this must be
    /// idempotent — the old ConnectionManager opened a new socket on each one.
    private func startPathMonitoringIfNeeded() {
        let shouldStart = lock.filWithLock {
            guard !isMonitoringPath else { return false }
            isMonitoringPath = true
            return true
        }
        guard shouldStart else { return }

        pathMonitor.pathUpdateHandler = { [weak self] path in
            guard path.status == .satisfied, let self else { return }
            // Connectivity is back: collapse any pending backoff rather than
            // making the user wait out a timer that is now pointless.
            for session in self.lock.filWithLock({ Array(self.sessions.values) }) {
                session.retryImmediatelyIfWaiting()
            }
        }
        pathMonitor.start(queue: DispatchQueue(label: "sh.fil.path-monitor"))
    }

    /// Called on scene phase transitions; see FilApp.
    func applicationDidEnterBackground() {
        for session in lock.filWithLock({ Array(sessions.values) }) {
            session.suspend()
        }
    }

    func applicationWillEnterForeground() {
        for session in lock.filWithLock({ Array(sessions.values) }) {
            session.resume()
        }
    }

    /// Force a fresh connection for a session that is already open.
    func reconnect(sessionId: String) {
        lock.filWithLock { sessions[sessionId] }?.reconnectNow()
    }

    /// The only teardown path.
    func close(sessionId: String) {
        let session = lock.filWithLock { sessions.removeValue(forKey: sessionId) }
        session?.shutdown()
    }

    func sendInput(sessionId: String, data: Data) {
        lock.filWithLock { sessions[sessionId] }?.sendInput(data)
    }

    func resize(sessionId: String, cols: UInt16, rows: UInt16) {
        lock.filWithLock { sessions[sessionId] }?.resize(cols: cols, rows: rows)
    }

    // MARK: - Test seams

    func existingSession(sessionId: String) -> TerminalSession? {
        lock.filWithLock { sessions[sessionId] }
    }

    func removeAllSessions() {
        let all = lock.filWithLock {
            let s = sessions
            sessions.removeAll()
            return s
        }
        for (_, session) in all {
            session.shutdown()
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
