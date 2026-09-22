import Foundation

/// What a terminal connection is doing, from the UI's point of view.
///
/// This replaces the previous `isConnected` boolean, which could not tell
/// "we are retrying, sit tight" apart from "this is dead, tap the button".
enum TerminalConnectionState: Equatable, Sendable {
    case connecting
    case connected
    /// An automatic retry is scheduled or in flight.
    case reconnecting(attempt: Int)
    /// Deliberately released because the app went to the background.
    case suspended
    /// Gave up; the user has to act.
    case unreachable(String)
}

extension TerminalConnectionState {
    /// True only when nothing is going to happen without the user.
    var needsUserAction: Bool {
        if case .unreachable = self { return true }
        return false
    }

    /// What the Live Activity should say. `suspended` deliberately keeps
    /// reporting `connected`: the session on the Mac is alive and well, it is
    /// only the phone that stepped away.
    var activityStatus: FilActivityStatus {
        switch self {
        case .connected, .suspended: .connected
        case .connecting, .reconnecting: .reconnecting
        case .unreachable: .stale
        }
    }

    var accessibilityDescription: String {
        switch self {
        case .connected: "connected"
        case .connecting: "connecting"
        case .reconnecting: "reconnecting"
        case .suspended: "suspended"
        case .unreachable: "disconnected"
        }
    }
}

/// The transport a `TerminalSession` drives. Exists so tests can run the
/// session state machine without opening a real QUIC connection.
protocol TerminalTransport: AnyObject, Sendable {
    /// Payload and its end cursor are committed together by the session.
    var onOutputReceived: (@Sendable (Data, UInt64) -> Void)? { get set }
    var onConnected: (@Sendable () -> Void)? { get set }
    var onDisconnected: (@Sendable () -> Void)? { get set }
    var onBetterPathAvailable: (@Sendable () -> Void)? { get set }

    func connect(sessionId: String, ticket: String?, resumeFrom: UInt64)
    func disconnect()
    func disconnect(completion: @escaping @Sendable () -> Void)
    func sendInput(_ data: Data)
    func sendResize(cols: UInt16, rows: UInt16)
    func setServerCertificate(_ certificate: Data)
}

extension TerminalTransport {
    func disconnect(completion: @escaping @Sendable () -> Void) {
        disconnect()
        completion()
    }
}

/// Retry pacing. Deliberately aggressive at the start: the overwhelmingly
/// common case is a phone coming back from the background, where the hub is
/// reachable immediately and any delay is felt as lag.
enum ReconnectPolicy {
    static let initialDelay: TimeInterval = 0.25
    static let maxDelay: TimeInterval = 8
    /// How long a connection may sit in `.waiting`/`.preparing` before we give
    /// up on it and build a new one. NWConnection provides no connect timeout.
    static let connectTimeout: TimeInterval = 6
    /// Hub rollouts can briefly lose the daemon's session inventory. Allow
    /// seven retries, but stop after the eighth missing-session response.
    static let maxMissingSessionResponses = 8

    static func delay(forAttempt attempt: Int) -> TimeInterval {
        let raw = initialDelay * pow(2, Double(max(0, attempt - 1)))
        let capped = min(raw, maxDelay)
        // Jitter so several sessions resuming together do not synchronise.
        return capped * Double.random(in: 0.85...1.15)
    }
}

extension QUICTerminalClient: TerminalTransport {}

/// Everything that belongs to one terminal session, owned outside the view
/// hierarchy and outside the TCA effect lifetime.
///
/// The bug this type exists to make unrepresentable: the output relay used to
/// be created and destroyed alongside the QUIC client, while the live
/// SwiftTerm view held a reference captured once in `makeUIView`. Any
/// reconnect therefore left the view bound to an orphaned relay — the terminal
/// showed "connected" and then never printed another byte. Here the relay is
/// created once per session and outlives every connection.
final class TerminalSession: @unchecked Sendable {
    let sessionId: String
    /// Created once, for the lifetime of the session. Never swapped.
    let outputRelay = TerminalOutputRelay()

    // This synchronous callback bridge cannot await an actor. The recursive
    // lock permits a transport to report failure synchronously from connect or
    // resize, while serializing validation, publication and activation.
    private let lock = NSRecursiveLock()
    private let makeTransport: @Sendable (String) -> TerminalTransport
    private let makeFallbackTransport: (@Sendable (String) -> TerminalTransport)?
    private let retryPrimaryAfterFallbackFailure: Bool
    private var prefersFallback = false
    /// HTTPS authenticates both the attach ticket and the QUIC certificate.
    /// Nil is used only by injected test transports; production fails closed.
    private let mintTicket: @Sendable (String) async throws -> SessionTicketResponse?

    private var transport: TerminalTransport?
    private var state: TerminalConnectionState = .connecting
    private var subscribers: [UUID: AsyncStream<TerminalConnectionState>.Continuation] = [:]
    /// Survives reconnects so the new connection can re-declare the geometry.
    private var lastKnownSize: (cols: UInt16, rows: UInt16)?
    private var attempt = 0
    /// Separate from backoff: path/foreground callbacks may reset `attempt`,
    /// but must not extend a genuinely missing session's grace indefinitely.
    private var missingSessionResponses = 0
    private var retryTask: Task<Void, Never>?
    private var watchdog: Task<Void, Never>?
    private var ticketTask: Task<Void, Never>?
    private let sleep: @Sendable (TimeInterval) async throws -> Void
    /// Set while the app is backgrounded, so a late callback cannot resurrect
    /// the connection we deliberately released.
    private var isSuspended = false
    private var isClosed = false
    private var hasBeenOpened = false
    private var isStarting = false
    private var generation: UInt64 = 0
    private var terminalFailure: TerminalConnectionState?
    /// Persisted across connections so a reattach replays only what the user
    /// has not already seen.
    private var resumeOffset: UInt64 = 0

    init(
        sessionId: String,
        makeTransport: @escaping @Sendable (String) -> TerminalTransport,
        makeFallbackTransport: (@Sendable (String) -> TerminalTransport)? = nil,
        retryPrimaryAfterFallbackFailure: Bool = false,
        mintTicket: @escaping @Sendable (String) async throws -> SessionTicketResponse?,
        sleep: @escaping @Sendable (TimeInterval) async throws -> Void = {
            try await Task.sleep(for: .seconds($0))
        }
    ) {
        self.sessionId = sessionId
        self.makeTransport = makeTransport
        self.makeFallbackTransport = makeFallbackTransport
        self.retryPrimaryAfterFallbackFailure = retryPrimaryAfterFallbackFailure
        self.mintTicket = mintTicket
        self.sleep = sleep
    }

    // MARK: - Subscription

    /// Subscribe to state changes. Cancelling the returned stream unsubscribes
    /// and does nothing else — in particular it does not tear the connection
    /// down, which is what used to destroy the relay on every reconnect.
    func subscribe() -> AsyncStream<TerminalConnectionState> {
        AsyncStream { continuation in
            let id = UUID()
            lock.filWithLock {
                guard !isClosed else { continuation.finish(); return }
                subscribers[id] = continuation
                continuation.yield(state)
            }
            continuation.onTermination = { [weak self] _ in
                self?.removeSubscriber(id)
            }
        }
    }

    private func removeSubscriber(_ id: UUID) {
        lock.filWithLock { _ = subscribers.removeValue(forKey: id) }
    }

    var subscriberCount: Int {
        lock.filWithLock { subscribers.count }
    }

    var currentState: TerminalConnectionState {
        lock.filWithLock { state }
    }

    // MARK: - Connection

    /// Idempotent: starts a connection only if none is live.
    func connectIfNeeded() {
        lock.filWithLock { hasBeenOpened = true }
        startConnection()
    }

    /// Drop the current connection and immediately open a fresh one, resetting
    /// the backoff. This is what the Reconnect button does.
    func reconnectNow() {
        reconnectNow(ifCurrent: nil)
    }

    private func reconnectNow(ifCurrent expected: TerminalTransport?) {
        let (allowed, old) = lock.filWithLock { () -> (Bool, TerminalTransport?) in
            guard hasBeenOpened, !isClosed, !isSuspended,
                  expected == nil || transport === expected else { return (false, nil) }
            let t = transport
            transport = nil
            generation &+= 1
            isStarting = false
            attempt = 0
            if expected == nil { missingSessionResponses = 0 }
            terminalFailure = nil
            cancelWatchdogLocked()
            ticketTask?.cancel()
            ticketTask = nil
            retryTask?.cancel()
            retryTask = nil
            return (true, t)
        }
        guard allowed else { return }
        old?.disconnect()
        startConnection()
    }

    /// Collapse any pending backoff and retry immediately — used when the
    /// network path becomes satisfied or the app returns to the foreground.
    func retryImmediatelyIfWaiting() {
        let shouldRetry = lock.filWithLock {
            guard hasBeenOpened, !isClosed, !isSuspended, !isStarting,
                  terminalFailure == nil, transport == nil else { return false }
            retryTask?.cancel()
            retryTask = nil
            attempt = 0
            return true
        }
        guard shouldRetry else { return }
        startConnection()
    }

    private func startConnection(ifGeneration expected: UInt64? = nil) {
        let reservation = lock.filWithLock { () -> (UInt64, Bool)? in
            guard hasBeenOpened, !isClosed, !isSuspended, !isStarting,
                  terminalFailure == nil, transport == nil,
                  expected == nil || generation == expected else { return nil }
            isStarting = true
            generation &+= 1
            return (generation, prefersFallback)
        }
        guard let (reserved, useFallback) = reservation else { return }
        let factory = useFallback ? (makeFallbackTransport ?? makeTransport) : makeTransport
        let client = factory(sessionId)

        client.onConnected = { [weak self, weak client] in
            guard let self, let client else { return }
            self.lock.filWithLock {
                guard self.generation == reserved, self.transport === client,
                      !self.isSuspended, !self.isClosed else { return }
                // A successful connection clears the backoff; without this the
                // delay ratchets up across a long-lived session.
                self.attempt = 0
                self.missingSessionResponses = 0
                self.cancelWatchdogLocked()
                self.transitionLocked(to: .connected)
                if let size = self.lastKnownSize {
                    client.sendResize(cols: size.cols, rows: size.rows)
                }
            }
        }
        client.onDisconnected = { [weak self, weak client] in
            guard let client else { return }
            self?.handleDisconnected(client)
        }
        client.onOutputReceived = { [weak self, weak client] data, offset in
            guard let self, let client else { return }
            self.lock.filWithLock {
                guard self.transport === client else { return }
                self.outputRelay.enqueue(data)
                self.resumeOffset = offset
            }
        }
        client.onBetterPathAvailable = { [weak self, weak client] in
            // Network.framework QUIC does not migrate; rebuild on the new path.
            guard let client else { return }
            self?.reconnectNow(ifCurrent: client)
        }

        // `attempt` counts consecutive failures, so it is incremented on
        // failure, not here. Zero means "this is a first try, not a retry".
        lock.filWithLock {
            guard generation == reserved, !isSuspended, !isClosed else { client.disconnect(); return }
            isStarting = false
            transport = client
            transitionLocked(to: attempt == 0 ? .connecting : .reconnecting(attempt: attempt))
            armWatchdog(for: client)

            // The ticket is fetched over HTTPS before the QUIC handshake. The
            // watchdog is already armed, so a hub that never answers still times
            // out into the normal retry path instead of hanging here.
            let mint = mintTicket
            let sid = sessionId
            ticketTask = Task { [weak self, weak client] in
                do {
                    let credential = try await mint(sid)
                    try Task.checkCancellation()
                    guard let self, let client else { return }
                    try self.lock.filWithLock {
                        guard self.generation == reserved, self.transport === client,
                              !self.isClosed, !self.isSuspended else { return }
                        if let credential {
                            guard !credential.ticket.isEmpty,
                                  let certificate = Data(base64Encoded: credential.quicCertificate), !certificate.isEmpty else {
                                throw HubError.invalidResponse
                            }
                            client.setServerCertificate(certificate)
                        }
                        // No shutdown can slip between this check and activation.
                        guard self.generation == reserved, self.transport === client else { return }
                        client.connect(sessionId: sid, ticket: credential?.ticket, resumeFrom: self.resumeOffset)
                    }
                } catch {
                    guard !Task.isCancelled, let self, let client else { return }
                    if case HubError.httpError(let status) = error, status == 401 || status == 403 || status == 404 {
                        let invalidated = self.lock.filWithLock { () -> Bool in
                            guard self.transport === client else { return false }
                            if status == 404 {
                                self.missingSessionResponses += 1
                                if self.missingSessionResponses < ReconnectPolicy.maxMissingSessionResponses {
                                    // Retire and schedule the retry under the
                                    // same identity check. 401/403 never enter
                                    // this grace path, even during a rollout.
                                    self.handleDisconnected(client)
                                    return false
                                }
                            }
                            self.transport = nil
                            self.generation &+= 1
                            self.cancelWatchdogLocked()
                            let failure = TerminalConnectionState.unreachable(status == 401 ? "Please sign in again." : "This session is no longer available.")
                            self.terminalFailure = failure
                            self.transitionLocked(to: failure)
                            return true
                        }
                        guard invalidated else { return }
                        client.disconnect()
                    } else {
                        self.handleDisconnected(client)
                    }
                }
            }
        }
    }

    /// NWConnection never leaves `.waiting` on its own, so a stalled connect
    /// has to be killed from the outside or the UI hangs on "Reconnecting"
    /// forever — which is exactly the reported symptom.
    private func armWatchdog(for client: TerminalTransport) {
        let sleep = sleep
        let task = Task { [weak self, weak client] in
            do { try await sleep(ReconnectPolicy.connectTimeout) } catch { return }
            guard !Task.isCancelled, let self, let client else { return }
            self.handleDisconnected(client, onlyIfConnecting: true)
        }
        lock.filWithLock {
            guard transport === client else { task.cancel(); return }
            watchdog?.cancel()
            watchdog = task
        }
    }

    private func cancelWatchdogLocked() {
        watchdog?.cancel()
        watchdog = nil
    }

    private func handleDisconnected(_ client: TerminalTransport, onlyIfConnecting: Bool = false) {
        let retired = lock.filWithLock { () -> Bool in
            guard transport === client, !isClosed, !isSuspended,
                  !onlyIfConnecting || state != .connected else { return false }
            transport = nil
            generation &+= 1
            cancelWatchdogLocked()
            ticketTask?.cancel()
            ticketTask = nil
            // Live sessions start on WSS, then try QUIC sequentially if WSS
            // fails. A failed alternate must not trap them on blocked UDP.
            // Keep the existing sticky-fallback behavior for injected clients.
            if makeFallbackTransport != nil {
                prefersFallback = retryPrimaryAfterFallbackFailure ? !prefersFallback : true
            }
            attempt += 1
            scheduleRetry(afterAttempt: attempt, generation: generation)
            return true
        }
        guard retired else { return }
        client.disconnect()
    }

    private func scheduleRetry(afterAttempt attempt: Int, generation expected: UInt64) {
        let delay = ReconnectPolicy.delay(forAttempt: attempt)
        transitionLocked(to: .reconnecting(attempt: attempt))

        let sleep = sleep
        let task = Task { [weak self] in
            do { try await sleep(delay) } catch { return }
            guard !Task.isCancelled, let self else { return }
            self.startConnection(ifGeneration: expected)
        }
        lock.filWithLock {
            guard generation == expected, !isClosed, !isSuspended else { task.cancel(); return }
            retryTask?.cancel()
            retryTask = task
        }
    }

    /// Release the connection but keep the session (and its relay, scrollback
    /// and declared size) alive.
    func suspend(completion: @escaping @Sendable () -> Void = {}) {
        let old = lock.filWithLock {
            guard !isClosed, !isSuspended else { return Optional<TerminalTransport>.none }
            let t = transport
            transport = nil
            generation &+= 1
            isStarting = false
            isSuspended = true
            retryTask?.cancel()
            retryTask = nil
            cancelWatchdogLocked()
            ticketTask?.cancel()
            ticketTask = nil
            transitionLocked(to: .suspended)
            return t
        }
        if let old { old.disconnect(completion: completion) } else { completion() }
    }

    /// Coming back to the foreground: no backoff, connect at once.
    func resume() {
        let shouldConnect = lock.filWithLock {
            guard !isClosed, isSuspended else { return false }
            isSuspended = false
            attempt = 0
            if let terminalFailure {
                transitionLocked(to: terminalFailure)
                return false
            }
            return transport == nil
        }
        guard shouldConnect else { return }
        startConnection()
    }

    /// Final teardown. Only the registry's `close` calls this.
    func shutdown() {
        let (old, subs) = lock.filWithLock {
            let t = transport
            let s = subscribers
            transport = nil
            generation &+= 1
            isStarting = false
            isClosed = true
            subscribers.removeAll()
            retryTask?.cancel()
            retryTask = nil
            cancelWatchdogLocked()
            ticketTask?.cancel()
            ticketTask = nil
            return (t, s)
        }
        old?.disconnect()
        for (_, continuation) in subs {
            continuation.finish()
        }
    }

    // MARK: - I/O

    func sendInput(_ data: Data) {
        lock.filWithLock { transport }?.sendInput(data)
    }

    func resize(cols: UInt16, rows: UInt16) {
        let client = lock.filWithLock {
            lastKnownSize = (cols, rows)
            return transport
        }
        client?.sendResize(cols: cols, rows: rows)
    }

    // MARK: - State

    /// Caller holds lock: observers see the same order as state mutations.
    private func transitionLocked(to newState: TerminalConnectionState) {
        guard !isClosed, state != newState else { return }
        state = newState
        for continuation in subscribers.values {
            continuation.yield(newState)
        }
    }
}
