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
}

/// The transport a `TerminalSession` drives. Exists so tests can run the
/// session state machine without opening a real QUIC connection.
protocol TerminalTransport: AnyObject, Sendable {
    var onDataReceived: (@Sendable (Data) -> Void)? { get set }
    var onConnected: (@Sendable () -> Void)? { get set }
    var onDisconnected: (@Sendable () -> Void)? { get set }

    func connect(sessionId: String)
    func disconnect()
    func sendInput(_ data: Data)
    func sendResize(cols: UInt16, rows: UInt16)
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

    /// Drop the current connection and immediately open a fresh one.
    func reconnectNow() {
        let old = lock.filWithLock {
            let t = transport
            transport = nil
            return t
        }
        old?.disconnect()
        startConnection()
    }

    private func startConnection() {
        let client = makeTransport(sessionId)

        client.onConnected = { [weak self] in
            guard let self else { return }
            // Re-declare the geometry: a fresh connection knows nothing about
            // the size the user is actually looking at.
            if let size = self.lock.filWithLock({ self.lastKnownSize }) {
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

        lock.filWithLock { transport = client }
        transition(to: .connecting)
        client.connect(sessionId: sessionId)
    }

    /// Overridden in phase 2 by the automatic retry policy. For now a dropped
    /// connection surfaces as `unreachable` and waits for the user.
    private func handleDisconnected() {
        let hadTransport = lock.filWithLock {
            let had = transport != nil
            transport = nil
            return had
        }
        guard hadTransport else { return }
        transition(to: .unreachable("Connection lost"))
    }

    /// Release the connection but keep the session (and its relay, scrollback
    /// and declared size) alive.
    func suspend() {
        let old = lock.filWithLock {
            let t = transport
            transport = nil
            return t
        }
        old?.disconnect()
        transition(to: .suspended)
    }

    /// Final teardown. Only the registry's `close` calls this.
    func shutdown() {
        let (old, subs) = lock.filWithLock {
            let t = transport
            let s = subscribers
            transport = nil
            subscribers.removeAll()
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
