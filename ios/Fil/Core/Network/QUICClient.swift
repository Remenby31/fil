import Foundation
import Network

final class QUICTerminalClient: @unchecked Sendable {
    private var connection: NWConnection?
    private let hubHost: String
    private let hubPort: UInt16
    private let stateLock = NSLock()
    private var isReady = false
    private var pendingResize: (cols: UInt16, rows: UInt16)?
    private var lastSentResize: (cols: UInt16, rows: UInt16)?

    var onDataReceived: (@Sendable (Data) -> Void)?
    var onConnected: (@Sendable () -> Void)?
    var onDisconnected: (@Sendable () -> Void)?
    /// NWConnection sits in `.waiting` indefinitely when the endpoint is
    /// unreachable — it never reaches `.failed` on its own. Measured: 94.5s
    /// against a black hole produced only `.preparing` and `.waiting`. The
    /// session uses this to arm a watchdog, which is the only way out.
    var onWaiting: (@Sendable (NWError) -> Void)?

    /// Guarantees `onDisconnected` fires at most once per connection. It could
    /// previously be delivered up to three times (`.failed`, `.cancelled`, and
    /// the receive loop), so a single drop produced a burst of state changes.
    private var didReportDisconnect = false
    /// Input typed before the stream is ready used to be dropped on the floor.
    private var pendingInput: [Data] = []
    private static let maxPendingInputBytes = 64 * 1024

    init(hubHost: String, hubPort: UInt16 = 16433) {
        self.hubHost = hubHost
        self.hubPort = hubPort
    }

    func connect(sessionId: String) {
        let params = NWParameters(quic: makeQUICOptions())

        let endpoint = NWEndpoint.hostPort(
            host: NWEndpoint.Host(hubHost),
            port: NWEndpoint.Port(rawValue: hubPort)!
        )

        let conn = NWConnection(to: endpoint, using: params)

        conn.stateUpdateHandler = { [weak self] state in
            self?.handleConnectionState(state, sessionId: sessionId)
        }
        // A connection can stay nominally `.ready` while carrying no traffic
        // after a network change; viability is how Network.framework says so.
        conn.viabilityUpdateHandler = { [weak self] isViable in
            guard !isViable else { return }
            self?.reportDisconnected()
        }
        // Wi-Fi <-> cellular handoff. Network.framework QUIC does not migrate
        // the connection for us, so the session has to build a new one.
        conn.betterPathUpdateHandler = { [weak self] betterPathAvailable in
            guard betterPathAvailable else { return }
            self?.onBetterPathAvailable?()
        }

        stateLock.filWithLock {
            connection = conn
            isReady = false
            lastSentResize = nil
            didReportDisconnect = false
            pendingInput.removeAll()
        }
        conn.start(queue: .global(qos: .userInteractive))
    }

    /// Signalled when the OS reports a better route; the session responds by
    /// standing up a replacement connection.
    var onBetterPathAvailable: (@Sendable () -> Void)?

    func disconnect() {
        let (conn, wasReady) = stateLock.filWithLock {
            let conn = connection
            let wasReady = isReady
            connection = nil
            isReady = false
            pendingResize = nil
            lastSentResize = nil
            pendingInput.removeAll()
            // A deliberate teardown must not look like a drop to the session.
            didReportDisconnect = true
            return (conn, wasReady)
        }
        guard let conn else { return }

        // Otherwise the handlers keep firing on a connection we have discarded.
        conn.stateUpdateHandler = nil
        conn.viabilityUpdateHandler = nil
        conn.betterPathUpdateHandler = nil

        guard wasReady else {
            conn.cancel()
            return
        }

        // Explicitly release the remote attachment before closing QUIC. This
        // restores the Mac PTY size immediately during normal navigation.
        conn.send(
            content: Data([TerminalControlFrame.detach]),
            contentContext: .defaultMessage,
            isComplete: true,
            completion: .contentProcessed { _ in conn.cancel() }
        )
    }

    func sendInput(_ data: Data) {
        guard !data.isEmpty else { return }
        let frame = makeInputFrame(data)
        let ready = stateLock.filWithLock { () -> Bool in
            if isReady { return true }
            // Buffer instead of dropping: keystrokes typed during the
            // sub-second window before the stream is ready used to vanish.
            let buffered = pendingInput.reduce(0) { $0 + $1.count }
            if buffered + frame.count <= Self.maxPendingInputBytes {
                pendingInput.append(frame)
            }
            return false
        }
        guard ready else { return }
        sendFrame(frame)
    }

    func sendResize(cols: UInt16, rows: UInt16) {
        guard cols > 0, rows > 0 else { return }
        let shouldSend = stateLock.filWithLock {
            pendingResize = (cols, rows)
            guard isReady,
                  lastSentResize?.cols != cols || lastSentResize?.rows != rows else {
                return false
            }
            lastSentResize = (cols, rows)
            return true
        }
        guard shouldSend else { return }
        sendFrame(makeResizeFrame(cols: cols, rows: rows))
    }

    private func handleConnectionState(_ state: NWConnection.State, sessionId: String) {
        switch state {
        case .ready:
            sendStreamHeader(sessionId: sessionId)
            let (pendingResize, queuedInput) = stateLock.filWithLock {
                isReady = true
                let resize = self.pendingResize
                if let resize {
                    lastSentResize = resize
                }
                let queued = pendingInput
                pendingInput.removeAll()
                return (resize, queued)
            }
            if let pendingResize {
                sendFrame(makeResizeFrame(cols: pendingResize.cols, rows: pendingResize.rows))
            }
            for frame in queuedInput {
                sendFrame(frame)
            }
            onConnected?()
            startReceiving()
        case .waiting(let error):
            // Not a failure as far as Network.framework is concerned: it will
            // keep waiting forever. Hand it to the session, which times out.
            onWaiting?(error)
        case .failed, .cancelled:
            reportDisconnected()
        case .preparing, .setup:
            break
        @unknown default:
            break
        }
    }

    /// Collapses the several teardown paths into exactly one notification.
    private func reportDisconnected() {
        let shouldReport = stateLock.filWithLock {
            isReady = false
            guard !didReportDisconnect else { return false }
            didReportDisconnect = true
            return true
        }
        guard shouldReport else { return }
        onDisconnected?()
    }

    private func sendStreamHeader(sessionId: String) {
        var header = Data([0x02])
        let sidData = Data(sessionId.utf8)
        var lenBytes = UInt16(sidData.count).bigEndian
        header.append(Data(bytes: &lenBytes, count: 2))
        header.append(sidData)

        let conn = stateLock.filWithLock { connection }
        conn?.send(content: header, completion: .contentProcessed { _ in })
    }

    private func startReceiving() {
        receiveLoop()
    }

    private func receiveLoop() {
        let conn = stateLock.filWithLock { connection }
        conn?.receive(minimumIncompleteLength: 1, maximumLength: 16384) { [weak self] data, _, isComplete, error in
            if let data, !data.isEmpty {
                self?.onDataReceived?(data)
            }
            if isComplete || error != nil {
                self?.reportDisconnected()
                return
            }
            self?.receiveLoop()
        }
    }

    private func makeQUICOptions() -> NWProtocolQUIC.Options {
        let options = NWProtocolQUIC.Options(alpn: ["fil"])
        // The hub also advertises this timeout. If iOS is killed or suspended
        // before sending the explicit detach frame, the Mac is released in
        // seconds instead of retaining the phone's PTY size for five minutes.
        options.idleTimeout = 15_000
        let secOptions = options.securityProtocolOptions
        sec_protocol_options_set_verify_block(secOptions, { _, _, completion in
            completion(true)
        }, .global(qos: .userInteractive))
        return options
    }

    private func sendFrame(_ frame: Data) {
        let conn = stateLock.filWithLock { isReady ? connection : nil }
        conn?.send(content: frame, completion: .contentProcessed { _ in })
    }

    private func makeInputFrame(_ data: Data) -> Data {
        var frame = Data([TerminalControlFrame.input])
        var length = UInt32(data.count).bigEndian
        frame.append(Data(bytes: &length, count: MemoryLayout<UInt32>.size))
        frame.append(data)
        return frame
    }

    private func makeResizeFrame(cols: UInt16, rows: UInt16) -> Data {
        var frame = Data([TerminalControlFrame.resize])
        var colsBE = cols.bigEndian
        var rowsBE = rows.bigEndian
        frame.append(Data(bytes: &colsBE, count: MemoryLayout<UInt16>.size))
        frame.append(Data(bytes: &rowsBE, count: MemoryLayout<UInt16>.size))
        return frame
    }
}

private enum TerminalControlFrame {
    static let input: UInt8 = 0x00
    static let resize: UInt8 = 0x01
    static let detach: UInt8 = 0x02
}
