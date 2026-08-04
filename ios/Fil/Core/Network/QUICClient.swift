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

        stateLock.filWithLock {
            connection = conn
            isReady = false
            lastSentResize = nil
        }
        conn.start(queue: .global(qos: .userInteractive))
    }

    func disconnect() {
        let (conn, wasReady) = stateLock.filWithLock {
            let conn = connection
            let wasReady = isReady
            connection = nil
            isReady = false
            pendingResize = nil
            lastSentResize = nil
            return (conn, wasReady)
        }
        guard let conn else { return }

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
        sendFrame(makeInputFrame(data))
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
            let pendingResize = stateLock.filWithLock {
                isReady = true
                let resize = self.pendingResize
                if let resize {
                    lastSentResize = resize
                }
                return resize
            }
            if let pendingResize {
                sendFrame(makeResizeFrame(cols: pendingResize.cols, rows: pendingResize.rows))
            }
            onConnected?()
            startReceiving()
        case .failed:
            stateLock.filWithLock { isReady = false }
            onDisconnected?()
        case .cancelled:
            stateLock.filWithLock { isReady = false }
            onDisconnected?()
        default:
            break
        }
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
                self?.onDisconnected?()
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
