import Foundation
import Network
import Security

/// The narrow Network.framework boundary used by deterministic transport tests.
protocol TerminalQUICConnection: AnyObject, Sendable {
    var stateUpdateHandler: (@Sendable (NWConnection.State) -> Void)? { get set }
    var viabilityUpdateHandler: (@Sendable (Bool) -> Void)? { get set }
    var betterPathUpdateHandler: (@Sendable (Bool) -> Void)? { get set }
    func start(queue: DispatchQueue)
    func cancel()
    func send(_ data: Data, isComplete: Bool, completion: @escaping @Sendable (NWError?) -> Void)
    func receive(completion: @escaping @Sendable (Data?, Bool, NWError?) -> Void)
}

private final class NetworkTerminalConnection: TerminalQUICConnection, @unchecked Sendable {
    private let connection: NWConnection
    init(endpoint: NWEndpoint, parameters: NWParameters) {
        connection = NWConnection(to: endpoint, using: parameters)
    }
    var stateUpdateHandler: (@Sendable (NWConnection.State) -> Void)? {
        get { connection.stateUpdateHandler }
        set { connection.stateUpdateHandler = newValue }
    }
    var viabilityUpdateHandler: (@Sendable (Bool) -> Void)? {
        get { connection.viabilityUpdateHandler }
        set { connection.viabilityUpdateHandler = newValue }
    }
    var betterPathUpdateHandler: (@Sendable (Bool) -> Void)? {
        get { connection.betterPathUpdateHandler }
        set { connection.betterPathUpdateHandler = newValue }
    }
    func start(queue: DispatchQueue) { connection.start(queue: queue) }
    func cancel() { connection.cancel() }
    func send(_ data: Data, isComplete: Bool, completion: @escaping @Sendable (NWError?) -> Void) {
        connection.send(content: data, contentContext: .defaultMessage,
                        isComplete: isComplete, completion: .contentProcessed(completion))
    }
    func receive(completion: @escaping @Sendable (Data?, Bool, NWError?) -> Void) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16384) { data, _, done, error in
            completion(data, done, error)
        }
    }
}

/// One instance per attach. All connection/decoder/input state is confined to
/// queue. Callback properties alone use callbackLock, including test injection.
/// Public operations enqueue, never synchronously wait on the network queue:
/// session callbacks can safely call back into this transport.
final class QUICTerminalClient: @unchecked Sendable {
    private let queue = DispatchQueue(label: "sh.fil.quic")
    private let callbackLock = NSLock()
    private var outputCallback: (@Sendable (Data, UInt64) -> Void)?
    private var connectedCallback: (@Sendable () -> Void)?
    private var disconnectedCallback: (@Sendable () -> Void)?
    private var betterPathCallback: (@Sendable () -> Void)?
    private var waitingCallback: (@Sendable (NWError) -> Void)?

    var onOutputReceived: (@Sendable (Data, UInt64) -> Void)? {
        get { callbackLock.filWithLock { outputCallback } }
        set { callbackLock.filWithLock { outputCallback = newValue } }
    }
    var onConnected: (@Sendable () -> Void)? {
        get { callbackLock.filWithLock { connectedCallback } }
        set { callbackLock.filWithLock { connectedCallback = newValue } }
    }
    var onDisconnected: (@Sendable () -> Void)? {
        get { callbackLock.filWithLock { disconnectedCallback } }
        set { callbackLock.filWithLock { disconnectedCallback = newValue } }
    }
    var onBetterPathAvailable: (@Sendable () -> Void)? {
        get { callbackLock.filWithLock { betterPathCallback } }
        set { callbackLock.filWithLock { betterPathCallback = newValue } }
    }
    var onWaiting: (@Sendable (NWError) -> Void)? {
        get { callbackLock.filWithLock { waitingCallback } }
        set { callbackLock.filWithLock { waitingCallback = newValue } }
    }

    private let hubHost: String
    private let hubPort: UInt16
    private let makeConnection: @Sendable (NWEndpoint, NWParameters) -> TerminalQUICConnection
    private let detachTimeout: TimeInterval
    private var connection: TerminalQUICConnection?
    private var hasStarted = false
    private var isClosed = false
    private var isReady = false
    private var headerSent = false
    private var decoder = TerminalStreamDecoder()
    private var serverCertificate = Data()
    private var pendingResize: (cols: UInt16, rows: UInt16)?
    private var lastSentResize: (cols: UInt16, rows: UInt16)?
    private var pendingInput: [Data] = []
    private var pendingInputBytes = 0
    private static let maxPendingInputBytes = 64 * 1024

    init(
        hubHost: String,
        hubPort: UInt16 = 16433,
        detachTimeout: TimeInterval = 1,
        makeConnection: @escaping @Sendable (NWEndpoint, NWParameters) -> TerminalQUICConnection = {
            NetworkTerminalConnection(endpoint: $0, parameters: $1)
        }
    ) {
        self.hubHost = hubHost
        self.hubPort = hubPort
        self.detachTimeout = detachTimeout
        self.makeConnection = makeConnection
    }

    deinit { connection?.cancel() }

    func setServerCertificate(_ certificate: Data) {
        queue.async { [self] in
            guard !hasStarted, !isClosed else { return }
            serverCertificate = certificate
        }
    }

    func connect(sessionId: String, ticket: String?, resumeFrom: UInt64 = 0) {
        queue.async { [self] in
            guard !hasStarted, !isClosed else { return }
            hasStarted = true
            guard let ticket, !ticket.isEmpty,
                  !serverCertificate.isEmpty, !hubHost.isEmpty,
                  let port = NWEndpoint.Port(rawValue: hubPort),
                  let header = Self.attachHeader(sessionId: sessionId, ticket: ticket, resumeFrom: resumeFrom) else {
                fail()
                return
            }
            let conn = makeConnection(
                .hostPort(host: NWEndpoint.Host(hubHost), port: port),
                NWParameters(quic: makeQUICOptions())
            )
            connection = conn
            conn.stateUpdateHandler = { [weak self, weak conn] state in
                guard let self, let conn else { return }
                self.queue.async { [weak self] in
                    guard let self, self.connection === conn, !self.isClosed else { return }
                    switch state {
                    case .ready:
                        guard !self.headerSent else { return }
                        self.headerSent = true
                        self.send(header, on: conn)
                        self.receive(on: conn)
                    case .waiting(let error): self.onWaiting?(error)
                    case .failed, .cancelled: self.fail()
                    default: break
                    }
                }
            }
            conn.viabilityUpdateHandler = { [weak self, weak conn] viable in
                guard !viable, let self, let conn else { return }
                self.queue.async { [weak self] in
                    guard let self, self.connection === conn else { return }
                    self.fail()
                }
            }
            conn.betterPathUpdateHandler = { [weak self, weak conn] better in
                guard better, let self, let conn else { return }
                self.queue.async { [weak self] in
                    guard let self, self.connection === conn, !self.isClosed else { return }
                    self.onBetterPathAvailable?()
                }
            }
            conn.start(queue: queue)
        }
    }

    func disconnect() { disconnect(completion: {}) }

    func disconnect(completion: @escaping @Sendable () -> Void) {
        queue.async { [self] in
            let conn = connection
            connection = nil
            let shouldDetach = isReady
            isClosed = true
            isReady = false
            pendingResize = nil
            pendingInput.removeAll()
            pendingInputBytes = 0
            guard let conn else { completion(); return }
            clearHandlers(conn)
            guard shouldDetach else { conn.cancel(); completion(); return }
            // A black-holed send may never complete. Bound both socket lifetime
            // and the application's background task, with exactly-once cleanup.
            let done = TerminalCompletionOnce {
                conn.cancel()
                completion()
            }
            queue.asyncAfter(deadline: .now() + detachTimeout) { done.run() }
            conn.send(Data([TerminalControlFrame.detach]), isComplete: true) { _ in done.run() }
        }
    }

    func sendInput(_ data: Data) {
        guard !data.isEmpty, data.count <= Int(UInt32.max) else { return }
        queue.async { [self] in
            guard !isClosed else { return }
            var frame = Data([TerminalControlFrame.input])
            var count = UInt32(data.count).bigEndian
            frame.append(Data(bytes: &count, count: 4))
            frame.append(data)
            if isReady, let connection {
                send(frame, on: connection)
            } else if pendingInputBytes + frame.count <= Self.maxPendingInputBytes {
                pendingInput.append(frame)
                pendingInputBytes += frame.count
            }
        }
    }

    func sendResize(cols: UInt16, rows: UInt16) {
        guard cols > 0, rows > 0 else { return }
        queue.async { [self] in
            guard !isClosed else { return }
            pendingResize = (cols, rows)
            flushResize()
        }
    }

    private func flushResize() {
        guard isReady, let connection, let size = pendingResize,
              lastSentResize?.cols != size.cols || lastSentResize?.rows != size.rows else { return }
        lastSentResize = size
        var frame = Data([TerminalControlFrame.resize])
        var cols = size.cols.bigEndian
        var rows = size.rows.bigEndian
        frame.append(Data(bytes: &cols, count: 2))
        frame.append(Data(bytes: &rows, count: 2))
        send(frame, on: connection)
    }

    private func receive(on conn: TerminalQUICConnection) {
        conn.receive { [weak self, weak conn] data, complete, error in
            guard let self, let conn else { return }
            self.queue.async { [weak self] in
                guard let self, self.connection === conn, !self.isClosed else { return }
                if let data, !data.isEmpty {
                    do {
                        let wasReady = self.decoder.isReady
                        let payload = try self.decoder.receive(data)
                        if let offset = self.decoder.offset {
                            // Commit bytes and cursor before announcing readiness.
                            self.onOutputReceived?(payload, offset)
                        }
                        if !wasReady && self.decoder.isReady {
                            self.isReady = true
                            self.onConnected?()
                            self.flushResize()
                            let input = self.pendingInput
                            self.pendingInput.removeAll()
                            self.pendingInputBytes = 0
                            for frame in input { self.send(frame, on: conn) }
                        }
                    } catch {
                        self.fail()
                        return
                    }
                }
                if complete || error != nil {
                    self.fail()
                } else {
                    self.receive(on: conn)
                }
            }
        }
    }

    private func send(_ data: Data, on conn: TerminalQUICConnection) {
        conn.send(data, isComplete: false) { [weak self, weak conn] error in
            guard error != nil, let self, let conn else { return }
            self.queue.async { [weak self] in
                guard let self, self.connection === conn else { return }
                self.fail()
            }
        }
    }

    private func fail() {
        guard !isClosed else { return }
        isClosed = true
        isReady = false
        let conn = connection
        connection = nil
        pendingInput.removeAll()
        pendingInputBytes = 0
        if let conn { clearHandlers(conn); conn.cancel() }
        onDisconnected?()
    }

    private func clearHandlers(_ conn: TerminalQUICConnection) {
        conn.stateUpdateHandler = nil
        conn.viabilityUpdateHandler = nil
        conn.betterPathUpdateHandler = nil
    }

    /// v3: [0x13][u16 sid length][sid][u16 ticket length][ticket][u64 cursor].
    static func attachHeader(sessionId: String, ticket: String, resumeFrom: UInt64) -> Data? {
        let sid = Data(sessionId.utf8)
        let token = Data(ticket.utf8)
        guard !sid.isEmpty, !token.isEmpty,
              let sidLength = UInt16(exactly: sid.count),
              let ticketLength = UInt16(exactly: token.count) else { return nil }
        var header = Data([0x13])
        var sidBE = sidLength.bigEndian
        var ticketBE = ticketLength.bigEndian
        var cursorBE = resumeFrom.bigEndian
        header.append(Data(bytes: &sidBE, count: 2))
        header.append(sid)
        header.append(Data(bytes: &ticketBE, count: 2))
        header.append(token)
        header.append(Data(bytes: &cursorBE, count: 8))
        return header
    }

    private func makeQUICOptions() -> NWProtocolQUIC.Options {
        let options = NWProtocolQUIC.Options(alpn: ["fil"])
        options.idleTimeout = 15_000
        let expected = serverCertificate
        sec_protocol_options_set_verify_block(options.securityProtocolOptions, { _, trust, completion in
            let trust = sec_trust_copy_ref(trust).takeRetainedValue()
            guard !expected.isEmpty,
                  let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
                  let leaf = chain.first else { completion(false); return }
            completion((SecCertificateCopyData(leaf) as Data) == expected)
        }, queue)
        return options
    }
}

/// Send completion and timeout may race, on different queues.
private final class TerminalCompletionOnce: @unchecked Sendable {
    private let lock = NSLock()
    private var action: (@Sendable () -> Void)?
    init(_ action: @escaping @Sendable () -> Void) { self.action = action }
    func run() {
        let action = lock.filWithLock {
            let action = self.action
            self.action = nil
            return action
        }
        action?()
    }
}


/// HTTPS/WSS uses URLSession's system certificate validation. No permissive
/// trust delegate and no bearer token in the URL or binary terminal frames.
protocol TerminalWebSocketTask: AnyObject, Sendable {
    func resume()
    func cancel(with closeCode: URLSessionWebSocketTask.CloseCode, reason: Data?)
    func send(_ message: URLSessionWebSocketTask.Message,
              completionHandler: @escaping @Sendable (Error?) -> Void)
    func receive(completionHandler: @escaping @Sendable (Result<URLSessionWebSocketTask.Message, Error>) -> Void)
    func sendPing(pongReceiveHandler: @escaping @Sendable (Error?) -> Void)
}

extension URLSessionWebSocketTask: TerminalWebSocketTask {}

/// Authenticated fallback for networks which block UDP. Queue-confined just
/// like QUIC; only callback registration needs a separate lock.
final class WebSocketTerminalClient: TerminalTransport, @unchecked Sendable {
    private struct Callbacks {
        var output: (@Sendable (Data, UInt64) -> Void)?
        var connected: (@Sendable () -> Void)?
        var disconnected: (@Sendable () -> Void)?
        var path: (@Sendable () -> Void)?
    }
    private let callbackLock = NSLock()
    private var callbacks = Callbacks()
    var onOutputReceived: (@Sendable (Data, UInt64) -> Void)? {
        get { callbackLock.filWithLock { callbacks.output } }
        set { callbackLock.filWithLock { callbacks.output = newValue } }
    }
    var onConnected: (@Sendable () -> Void)? {
        get { callbackLock.filWithLock { callbacks.connected } }
        set { callbackLock.filWithLock { callbacks.connected = newValue } }
    }
    var onDisconnected: (@Sendable () -> Void)? {
        get { callbackLock.filWithLock { callbacks.disconnected } }
        set { callbackLock.filWithLock { callbacks.disconnected = newValue } }
    }
    var onBetterPathAvailable: (@Sendable () -> Void)? {
        get { callbackLock.filWithLock { callbacks.path } }
        set { callbackLock.filWithLock { callbacks.path = newValue } }
    }

    private let queue = DispatchQueue(label: "sh.fil.terminal-websocket")
    private let hubURL: String
    private let token: String?
    private let makeSocket: @Sendable (URLRequest) -> TerminalWebSocketTask
    private let detachTimeout: TimeInterval
    private var socket: TerminalWebSocketTask?
    private var hasStarted = false
    private var isClosed = false
    private var isReady = false
    private var decoder = TerminalStreamDecoder()
    private var pinger: DispatchSourceTimer?
    private var pendingInput: [Data] = []
    private var pendingInputBytes = 0
    private var pendingResize: (UInt16, UInt16)?
    private var lastSentResize: (UInt16, UInt16)?

    init(
        hubURL: String,
        token: String?,
        detachTimeout: TimeInterval = 1,
        makeSocket: @escaping @Sendable (URLRequest) -> TerminalWebSocketTask = {
            URLSession.shared.webSocketTask(with: $0)
        }
    ) {
        self.hubURL = hubURL
        self.token = token
        self.detachTimeout = detachTimeout
        self.makeSocket = makeSocket
    }

    deinit {
        pinger?.cancel()
        socket?.cancel(with: .goingAway, reason: nil)
    }

    // WSS authenticates the server using system HTTPS trust, not the QUIC pin.
    func setServerCertificate(_ certificate: Data) {}

    static func request(hubURL: String, token: String?, sessionId: String, resumeFrom: UInt64) -> URLRequest? {
        guard let token, !token.isEmpty, !sessionId.isEmpty,
              var url = URLComponents(string: hubURL),
              url.scheme?.lowercased() == "https", url.host != nil,
              url.user == nil, url.password == nil else { return nil }
        url.scheme = "wss"
        url.path = "/ws/data/\(sessionId)"
        url.queryItems = [
            URLQueryItem(name: "role", value: "client"),
            URLQueryItem(name: "resume_from", value: String(resumeFrom))
        ]
        url.fragment = nil
        guard let endpoint = url.url else { return nil }
        var request = URLRequest(url: endpoint)
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        return request
    }

    func connect(sessionId: String, ticket: String?, resumeFrom: UInt64) {
        queue.async { [self] in
            guard !hasStarted, !isClosed else { return }
            hasStarted = true
            guard let request = Self.request(hubURL: hubURL, token: token, sessionId: sessionId, resumeFrom: resumeFrom) else {
                fail()
                return
            }
            let socket = makeSocket(request)
            self.socket = socket
            socket.resume()
            receive(on: socket)
            let timer = DispatchSource.makeTimerSource(queue: queue)
            timer.schedule(deadline: .now() + 30, repeating: 30)
            timer.setEventHandler { [weak self, weak socket] in
                guard let self, let socket, self.socket === socket else { return }
                socket.sendPing { [weak self, weak socket] error in
                    guard error != nil, let self, let socket else { return }
                    self.queue.async { [weak self] in
                        guard let self, self.socket === socket else { return }
                        self.fail()
                    }
                }
            }
            pinger = timer
            timer.resume()
        }
    }

    func disconnect() { disconnect(completion: {}) }

    func disconnect(completion: @escaping @Sendable () -> Void) {
        queue.async { [self] in
            let socket = self.socket
            self.socket = nil
            let shouldDetach = isReady
            isClosed = true
            isReady = false
            pinger?.cancel()
            pinger = nil
            pendingInput.removeAll()
            pendingInputBytes = 0
            guard let socket else { completion(); return }
            guard shouldDetach else {
                socket.cancel(with: .goingAway, reason: nil)
                completion()
                return
            }
            let done = TerminalCompletionOnce {
                socket.cancel(with: .normalClosure, reason: nil)
                completion()
            }
            queue.asyncAfter(deadline: .now() + detachTimeout) { done.run() }
            socket.send(.data(Data([TerminalControlFrame.detach]))) { _ in done.run() }
        }
    }

    func sendInput(_ data: Data) {
        guard !data.isEmpty, data.count <= Int(UInt32.max) else { return }
        queue.async { [self] in
            guard !isClosed else { return }
            var frame = Data([TerminalControlFrame.input])
            var count = UInt32(data.count).bigEndian
            frame.append(Data(bytes: &count, count: 4))
            frame.append(data)
            if isReady, let socket {
                send(frame, on: socket)
            } else if pendingInputBytes + frame.count <= 64 * 1024 {
                pendingInput.append(frame)
                pendingInputBytes += frame.count
            }
        }
    }

    func sendResize(cols: UInt16, rows: UInt16) {
        guard cols > 0, rows > 0 else { return }
        queue.async { [self] in
            guard !isClosed else { return }
            pendingResize = (cols, rows)
            flushResize()
        }
    }

    private func flushResize() {
        guard isReady, let socket, let size = pendingResize,
              lastSentResize?.0 != size.0 || lastSentResize?.1 != size.1 else { return }
        lastSentResize = size
        var frame = Data([TerminalControlFrame.resize])
        var cols = size.0.bigEndian
        var rows = size.1.bigEndian
        frame.append(Data(bytes: &cols, count: 2))
        frame.append(Data(bytes: &rows, count: 2))
        send(frame, on: socket)
    }

    private func receive(on socket: TerminalWebSocketTask) {
        socket.receive { [weak self, weak socket] result in
            guard let self, let socket else { return }
            self.queue.async { [weak self] in
                guard let self, self.socket === socket, !self.isClosed else { return }
                do {
                    guard case .data(let data) = try result.get() else { throw HubError.invalidResponse }
                    let wasReady = self.decoder.isReady
                    let payload = try self.decoder.receive(data)
                    if let offset = self.decoder.offset { self.onOutputReceived?(payload, offset) }
                    if !wasReady && self.decoder.isReady {
                        self.isReady = true
                        self.onConnected?()
                        self.flushResize()
                        let input = self.pendingInput
                        self.pendingInput.removeAll()
                        self.pendingInputBytes = 0
                        for frame in input { self.send(frame, on: socket) }
                    }
                    self.receive(on: socket)
                } catch { self.fail() }
            }
        }
    }

    private func send(_ data: Data, on socket: TerminalWebSocketTask) {
        socket.send(.data(data)) { [weak self, weak socket] error in
            guard error != nil, let self, let socket else { return }
            self.queue.async { [weak self] in
                guard let self, self.socket === socket else { return }
                self.fail()
            }
        }
    }

    private func fail() {
        guard !isClosed else { return }
        isClosed = true
        isReady = false
        pinger?.cancel()
        pinger = nil
        socket?.cancel(with: .goingAway, reason: nil)
        socket = nil
        pendingInput.removeAll()
        pendingInputBytes = 0
        onDisconnected?()
    }
}

/// v3 sends the offset of the first payload byte, not the end of the snapshot.
/// Advancing only on delivered bytes prevents both duplicate live output and
/// loss when a connection closes halfway through replaying its history.
struct TerminalStreamDecoder {
    private var prefix = Data()
    private(set) var offset: UInt64?
    var isReady: Bool { offset != nil }

    mutating func receive(_ data: Data) throws -> Data {
        var payload = data
        if offset == nil {
            prefix.append(data)
            guard prefix.count >= 8 else { return Data() }
            offset = prefix.prefix(8).reduce(UInt64(0)) { ($0 << 8) | UInt64($1) }
            payload = Data(prefix.dropFirst(8))
            prefix.removeAll()
        }
        let (next, overflow) = offset!.addingReportingOverflow(UInt64(payload.count))
        guard !overflow else { throw HubError.invalidResponse }
        offset = next
        return payload
    }
}

private enum TerminalControlFrame {
    static let input: UInt8 = 0x00
    static let resize: UInt8 = 0x01
    static let detach: UInt8 = 0x02
}
