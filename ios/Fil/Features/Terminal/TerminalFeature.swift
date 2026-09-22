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
        var followRevision: UInt64 = 0
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
        case connectionStateChanged(sessionId: String, state: TerminalConnectionState)
        case inputSent(Data)
        case dismiss
        case fontSizeChanged(CGFloat)
        case terminalSizeChanged(cols: Int, rows: Int)
        case latencyUpdated(Int)
        case reconnectTapped
        case followTapped
        case refreshFollowingStatus
        case followingStatusLoaded(sessionId: String, revision: UInt64, isFollowing: Bool)
        case followingChanged(sessionId: String, revision: UInt64, isFollowing: Bool, requestedFollow: Bool)
        case dismissLiveActivityUnavailableAlert
        case nextSession
        case previousSession
        case switchSession(String)
    }

    @Dependency(\.terminalClient) private var terminalClient
    @Dependency(\.terminalFollowClient) private var followClient

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear:
                let session = state.session
                let sessionId = session.id
                let hubHost = Self.quicHost()

                return .merge(
                    refreshFollowingStatus(&state),
                    .run { send in
                        for await connectionState in terminalClient.open(sessionId, hubHost) {
                            await send(.connectionStateChanged(sessionId: sessionId, state: connectionState))
                        }
                    }
                    .cancellable(id: CancelID.quic, cancelInFlight: true)
                )

            case .onDisappear, .dismiss:
                let sessionId = state.session.id
                // Close synchronously so a delayed effect cannot close a
                // replacement opened by a rapid disappear/appear sequence.
                terminalClient.close(sessionId)
                state.followRevision &+= 1
                state.isFollowRequestInFlight = false
                return .merge(
                    .cancel(id: CancelID.quic),
                    .cancel(id: CancelID.followStatus)
                )

            case let .connectionStateChanged(sessionId, connectionState):
                guard sessionId == state.session.id else { return .none }
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
                state.followRevision &+= 1
                let revision = state.followRevision

                let shouldFollow = !state.isFollowing
                let session = state.session
                let machineName = state.machineName
                let otherSessionCount = state.otherSessionCount
                let status: FilActivityStatus = state.isConnected ? .connected : .reconnecting

                return .run { send in
                    let following = await followClient.setFollowing(shouldFollow, session, machineName, otherSessionCount, status)
                    await send(.followingChanged(sessionId: session.id, revision: revision,
                                                 isFollowing: following, requestedFollow: shouldFollow))
                }

            case .refreshFollowingStatus:
                return refreshFollowingStatus(&state)

            case let .followingStatusLoaded(sessionId, revision, isFollowing):
                guard sessionId == state.session.id, revision == state.followRevision,
                      !state.isFollowRequestInFlight else { return .none }
                state.isFollowing = isFollowing
                return .none

            case let .followingChanged(sessionId, revision, isFollowing, requestedFollow):
                guard sessionId == state.session.id, revision == state.followRevision else { return .none }
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

    private enum CancelID { case quic, followStatus }

    private func refreshFollowingStatus(_ state: inout State) -> Effect<Action> {
        guard !state.isFollowRequestInFlight else { return .none }
        state.followRevision &+= 1
        let revision = state.followRevision
        let sessionId = state.session.id
        return .run { send in
            let following = await followClient.isFollowing(sessionId)
            await send(.followingStatusLoaded(sessionId: sessionId, revision: revision, isFollowing: following))
        }
        .cancellable(id: CancelID.followStatus, cancelInFlight: true)
    }

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
        state.isFollowRequestInFlight = false
        state.followRevision &+= 1
        state.showLiveActivityUnavailableAlert = false
        state.showDisconnectedAlert = false
        state.latencyMs = nil

        terminalClient.close(previousSessionId)
        return .concatenate(
            .cancel(id: CancelID.quic),
            .cancel(id: CancelID.followStatus),
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

struct TerminalClientDependency: Sendable {
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
    static let shared = live()

    /// Keep production transport ordering in one testable builder. The
    /// injected initializer below retains its existing factory signatures.
    static func live(
        monitorsNetwork: Bool = true,
        makeWebSocket: @escaping @Sendable () -> TerminalTransport = {
            WebSocketTerminalClient(hubURL: TokenStorage.loadHubUrl(), token: TokenStorage.loadToken())
        },
        makeQUIC: @escaping @Sendable (String) -> TerminalTransport = {
            QUICTerminalClient(hubHost: $0)
        },
        ticketProvider: @escaping @Sendable (String) async throws -> SessionTicketResponse? = {
            try await TerminalConnectionRegistry.mintTicket(sessionId: $0)
        }
    ) -> TerminalConnectionRegistry {
        TerminalConnectionRegistry(
            monitorsNetwork: monitorsNetwork,
            makeTransport: { _, _ in makeWebSocket() },
            makeHostFallbackTransport: { _, host in makeQUIC(host) },
            retryPrimaryAfterFallbackFailure: true,
            ticketProvider: ticketProvider
        )
    }

    private let lock = NSRecursiveLock()
    private var sessions: [String: TerminalSession] = [:]
    private var hubHosts: [String: String] = [:]
    private let pathMonitor = NWPathMonitor()
    private var isMonitoringPath = false
    private var isBackgrounded = false
    private let monitorsNetwork: Bool

    /// Immutable dependencies keep test registries isolated from the live one.
    private let makeTransport: @Sendable (_ sessionId: String, _ hubHost: String) -> TerminalTransport
    private let makeFallbackTransport: (@Sendable (String, String) -> TerminalTransport)?
    private let retryPrimaryAfterFallbackFailure: Bool
    private let ticketProvider: @Sendable (String) async throws -> SessionTicketResponse?

    init(
        monitorsNetwork: Bool = true,
        makeTransport: @escaping @Sendable (String, String) -> TerminalTransport = { _, host in
            QUICTerminalClient(hubHost: host)
        },
        makeFallbackTransport: (@Sendable (String) -> TerminalTransport)? = nil,
        makeHostFallbackTransport: (@Sendable (String, String) -> TerminalTransport)? = nil,
        retryPrimaryAfterFallbackFailure: Bool = false,
        ticketProvider: @escaping @Sendable (String) async throws -> SessionTicketResponse? = {
            try await TerminalConnectionRegistry.mintTicket(sessionId: $0)
        }
    ) {
        self.monitorsNetwork = monitorsNetwork
        self.makeTransport = makeTransport
        if let makeHostFallbackTransport {
            self.makeFallbackTransport = makeHostFallbackTransport
        } else if let makeFallbackTransport {
            self.makeFallbackTransport = { sid, _ in makeFallbackTransport(sid) }
        } else {
            self.makeFallbackTransport = nil
        }
        self.retryPrimaryAfterFallbackFailure = retryPrimaryAfterFallbackFailure
        self.ticketProvider = ticketProvider
    }

    deinit {
        pathMonitor.cancel()
        for session in sessions.values { session.shutdown() }
    }

    /// The relay for a session, created with the session and stable for its
    /// whole life. `makeUIView` can run before `open`, so this creates the
    /// session record on demand.
    func outputRelay(sessionId: String) -> TerminalOutputRelay {
        session(sessionId: sessionId, hubHost: nil).outputRelay
    }

    private func session(sessionId: String, hubHost: String?) -> TerminalSession {
        lock.filWithLock {
            if let hubHost { hubHosts[sessionId] = hubHost }
            if let existing = sessions[sessionId] {
                return existing
            }
            let ticketProvider = ticketProvider
            let created = TerminalSession(
                sessionId: sessionId,
                makeTransport: transportFactory(makeTransport),
                makeFallbackTransport: makeFallbackTransport.map { transportFactory($0) },
                retryPrimaryAfterFallbackFailure: retryPrimaryAfterFallbackFailure,
                mintTicket: ticketProvider
            )
            if isBackgrounded { created.suspend() }
            sessions[sessionId] = created
            return created
        }
    }

    /// Resolve the host at attempt time: the view may create the relay before
    /// open supplies the real host. Both primary and alternate use this path.
    private func transportFactory(
        _ make: @escaping @Sendable (String, String) -> TerminalTransport
    ) -> @Sendable (String) -> TerminalTransport {
        { [weak self] sid in
            let host = self?.lock.filWithLock { self?.hubHosts[sid] ?? "" } ?? ""
            return make(sid, host)
        }
    }

    /// Subscribe to a session's state, connecting it if it is not already up.
    /// Idempotent: repeated calls attach another subscriber to the same live
    /// connection instead of tearing it down and rebuilding it.
    func open(sessionId: String, hubHost: String) -> AsyncStream<TerminalConnectionState> {
        lock.filWithLock {
            let session = session(sessionId: sessionId, hubHost: hubHost)
            let stream = session.subscribe()
            session.connectIfNeeded()
            startPathMonitoringIfNeeded()
            return stream
        }
    }

    /// One monitor for the whole app. `pathUpdateHandler` fires on every path
    /// change, including redundant `.satisfied` callbacks, so this must be
    /// idempotent — the old ConnectionManager opened a new socket on each one.
    private func startPathMonitoringIfNeeded() {
        let shouldStart = lock.filWithLock {
            guard monitorsNetwork, !isMonitoringPath else { return false }
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
    func applicationDidEnterBackground(completion: @escaping @Sendable () -> Void = {}) {
        let detachments = DispatchGroup()
        lock.filWithLock {
            isBackgrounded = true
            for session in sessions.values {
                detachments.enter()
                session.suspend { detachments.leave() }
            }
        }
        detachments.notify(queue: .main, execute: completion)
    }

    func applicationWillEnterForeground() {
        lock.filWithLock {
            isBackgrounded = false
            for session in sessions.values { session.resume() }
        }
    }

    /// Force a fresh connection for a session that is already open.
    func reconnect(sessionId: String) {
        lock.filWithLock { sessions[sessionId] }?.reconnectNow()
    }

    /// The only teardown path.
    func close(sessionId: String) {
        lock.filWithLock {
            hubHosts.removeValue(forKey: sessionId)
            sessions.removeValue(forKey: sessionId)?.shutdown()
        }
    }

    func sendInput(sessionId: String, data: Data) {
        lock.filWithLock { sessions[sessionId] }?.sendInput(data)
    }

    func resize(sessionId: String, cols: UInt16, rows: UInt16) {
        lock.filWithLock { sessions[sessionId] }?.resize(cols: cols, rows: rows)
    }

    private static func mintTicket(sessionId: String) async throws -> SessionTicketResponse? {
        guard let token = TokenStorage.loadToken() else { throw HubError.httpError(401) }
        return try await HubClient().sessionTicket(sessionId: sessionId, token: token)
    }

    // MARK: - Test seams

    func existingSession(sessionId: String) -> TerminalSession? {
        lock.filWithLock { sessions[sessionId] }
    }

    func removeAllSessions() {
        let all = lock.filWithLock {
            let s = sessions
            sessions.removeAll()
            hubHosts.removeAll()
            return s
        }
        for (_, session) in all {
            session.shutdown()
        }
    }
}

extension DependencyValues {
    var terminalClient: TerminalClientDependency {
        get { self[TerminalClientDependency.self] }
        set { self[TerminalClientDependency.self] = newValue }
    }

    var terminalFollowClient: TerminalFollowClient {
        get { self[TerminalFollowClient.self] }
        set { self[TerminalFollowClient.self] = newValue }
    }
}

struct TerminalFollowClient: Sendable, DependencyKey {
    var isFollowing: @Sendable (String) async -> Bool
    var setFollowing: @Sendable (Bool, Session, String, Int, FilActivityStatus) async -> Bool

    static let liveValue = Self(
        isFollowing: { sessionId in
            if #available(iOS 16.2, *) { return await FilActivityManager.shared.isFollowing(sessionId: sessionId) }
            return false
        },
        setFollowing: { shouldFollow, session, machineName, otherSessionCount, status in
            guard #available(iOS 16.2, *) else { return false }
            if shouldFollow {
                return await FilActivityManager.shared.follow(session: session, machineName: machineName,
                                                            otherSessionCount: otherSessionCount, status: status)
            }
            await FilActivityManager.shared.stopFollowing(sessionId: session.id)
            return false
        }
    )
    static let testValue = Self(isFollowing: { _ in false }, setFollowing: { _, _, _, _, _ in false })
}

extension NSLocking {
    func filWithLock<T>(_ body: () throws -> T) rethrows -> T {
        lock()
        defer { unlock() }
        return try body()
    }
}
