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
    var onDataReceived: (@Sendable (Data) -> Void)? { get set }
    var onConnected: (@Sendable () -> Void)? { get set }
    var onDisconnected: (@Sendable () -> Void)? { get set }
    var onBetterPathAvailable: (@Sendable () -> Void)? { get set }

    func connect(sessionId: String)
    func disconnect()
    func sendInput(_ data: Data)
    func sendResize(cols: UInt16, rows: UInt16)
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

    private let lock = NSLock()
    private let makeTransport: @Sendable (String) -> TerminalTransport

    private var transport: TerminalTransport?
    private var state: TerminalConnectionState = .connecting
    private var subscribers: [UUID: AsyncStream<TerminalConnectionState>.Continuation] = [:]
    /// Survives reconnects so the new connection can re-declare the geometry.
    private var lastKnownSize: (cols: UInt16, rows: UInt16)?
    private var attempt = 0
    private var retryTask: Task<Void, Never>?
    private var watchdog: Task<Void, Never>?
    /// Set while the app is backgrounded, so a late callback cannot resurrect
    /// the connection we deliberately released.
    private var isSuspended = false

    init(sessionId: String, makeTransport: @escaping @Sendable (String) -> TerminalTransport) {
        self.sessionId = sessionId
        self.makeTransport = makeTransport
    }

    // MARK: - Subscription

    /// Subscribe to state changes. Cancelling the returned stream unsubscribes
    /// and does nothing else — in particular it does not tear the connection
    /// down, which is what used to destroy the relay on every reconnect.
    func subscribe() -> AsyncStream<TerminalConnectionState> {
        AsyncStream { continuation in
            let id = UUID()
            let current = lock.filWithLock {
                subscribers[id] = continuation
                return state
            }
            continuation.yield(current)
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
        let shouldConnect = lock.filWithLock { transport == nil }
        guard shouldConnect else { return }
        startConnection()
    }

    /// Drop the current connection and immediately open a fresh one, resetting
    /// the backoff. This is what the Reconnect button does.
    func reconnectNow() {
        let old = lock.filWithLock {
            let t = transport
            transport = nil
            attempt = 0
            isSuspended = false
            retryTask?.cancel()
            retryTask = nil
            return t
        }
        old?.disconnect()
        startConnection()
    }

    /// Collapse any pending backoff and retry immediately — used when the
    /// network path becomes satisfied or the app returns to the foreground.
    func retryImmediatelyIfWaiting() {
        let shouldRetry = lock.filWithLock {
            guard !isSuspended, transport == nil else { return false }
            retryTask?.cancel()
            retryTask = nil
            attempt = 0
            return true
        }
        guard shouldRetry else { return }
        startConnection()
    }

    private func startConnection() {
        let client = makeTransport(sessionId)

        client.onConnected = { [weak self] in
            guard let self else { return }
            let size = self.lock.filWithLock { () -> (cols: UInt16, rows: UInt16)? in
                // A successful connection clears the backoff; without this the
                // delay ratchets up across a long-lived session.
                self.attempt = 0
                self.cancelWatchdogLocked()
                return self.lastKnownSize
            }
            // Re-declare the geometry: a fresh connection knows nothing about
            // the size the user is actually looking at.
            if let size {
                client.sendResize(cols: size.cols, rows: size.rows)
            }
            self.transition(to: .connected)
        }
        client.onDisconnected = { [weak self] in
            self?.handleDisconnected()
        }
        client.onDataReceived = { [weak self] data in
            self?.outputRelay.enqueue(data)
        }
        client.onBetterPathAvailable = { [weak self] in
            // Network.framework QUIC does not migrate; rebuild on the new path.
            self?.reconnectNow()
        }

        // `attempt` counts consecutive failures, so it is incremented on
        // failure, not here. Zero means "this is a first try, not a retry".
        let failuresSoFar = lock.filWithLock {
            transport = client
            return attempt
        }
        transition(to: failuresSoFar == 0 ? .connecting : .reconnecting(attempt: failuresSoFar))
        armWatchdog(for: client)
        client.connect(sessionId: sessionId)
    }

    /// NWConnection never leaves `.waiting` on its own, so a stalled connect
    /// has to be killed from the outside or the UI hangs on "Reconnecting"
    /// forever — which is exactly the reported symptom.
    private func armWatchdog(for client: TerminalTransport) {
        let task = Task { [weak self, weak client] in
            try? await Task.sleep(nanoseconds: UInt64(ReconnectPolicy.connectTimeout * 1_000_000_000))
            guard !Task.isCancelled, let self else { return }
            let isStillPending = self.lock.filWithLock {
                self.transport === client && self.state != .connected
            }
            guard isStillPending else { return }
            client?.disconnect()
            self.handleDisconnected()
        }
        lock.filWithLock {
            watchdog?.cancel()
            watchdog = task
        }
    }

    private func cancelWatchdogLocked() {
        watchdog?.cancel()
        watchdog = nil
    }

    private func handleDisconnected() {
        let (hadTransport, nextAttempt, suspended) = lock.filWithLock {
            let had = transport != nil
            transport = nil
            cancelWatchdogLocked()
            if had { attempt += 1 }
            return (had, attempt, isSuspended)
        }
        guard hadTransport, !suspended else { return }
        scheduleRetry(afterAttempt: nextAttempt)
    }

    private func scheduleRetry(afterAttempt attempt: Int) {
        let delay = ReconnectPolicy.delay(forAttempt: attempt)
        transition(to: .reconnecting(attempt: attempt))

        let task = Task { [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            guard !Task.isCancelled, let self else { return }
            let shouldStart = self.lock.filWithLock { !self.isSuspended && self.transport == nil }
            guard shouldStart else { return }
            self.startConnection()
        }
        lock.filWithLock {
            retryTask?.cancel()
            retryTask = task
        }
    }

    /// Release the connection but keep the session (and its relay, scrollback
    /// and declared size) alive.
    func suspend() {
        let old = lock.filWithLock {
            let t = transport
            transport = nil
            isSuspended = true
            retryTask?.cancel()
            retryTask = nil
            cancelWatchdogLocked()
            return t
        }
        old?.disconnect()
        transition(to: .suspended)
    }

    /// Coming back to the foreground: no backoff, connect at once.
    func resume() {
        let shouldConnect = lock.filWithLock {
            guard isSuspended else { return false }
            isSuspended = false
            attempt = 0
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
            subscribers.removeAll()
            retryTask?.cancel()
            retryTask = nil
            cancelWatchdogLocked()
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

    private func transition(to newState: TerminalConnectionState) {
        let subs = lock.filWithLock { () -> [AsyncStream<TerminalConnectionState>.Continuation] in
            guard state != newState else { return [] }
            state = newState
            return Array(subscribers.values)
        }
        for continuation in subs {
            continuation.yield(newState)
        }
    }
}
