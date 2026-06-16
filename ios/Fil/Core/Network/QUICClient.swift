import Foundation
import Network

final class QUICTerminalClient: @unchecked Sendable {
    private var connection: NWConnection?
    private let hubHost: String
    private let hubPort: UInt16

    var onDataReceived: (@Sendable (Data) -> Void)?
    var onConnected: (@Sendable () -> Void)?
    var onDisconnected: (@Sendable () -> Void)?

    init(hubHost: String, hubPort: UInt16 = 4433) {
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

        conn.start(queue: .global(qos: .userInteractive))
        self.connection = conn
    }

    func disconnect() {
        connection?.cancel()
        connection = nil
    }

    func sendInput(_ data: Data) {
        connection?.send(content: data, completion: .contentProcessed { _ in })
    }

    private func handleConnectionState(_ state: NWConnection.State, sessionId: String) {
        switch state {
        case .ready:
            onConnected?()
            sendStreamHeader(sessionId: sessionId)
            startReceiving()
        case .failed:
            onDisconnected?()
        case .cancelled:
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

        connection?.send(content: header, completion: .contentProcessed { _ in })
    }

    private func startReceiving() {
        receiveLoop()
    }

    private func receiveLoop() {
        connection?.receive(minimumIncompleteLength: 1, maximumLength: 16384) { [weak self] data, _, isComplete, error in
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
        let options = NWProtocolQUIC.Options()
        let secOptions = options.securityProtocolOptions
        sec_protocol_options_set_verify_block(secOptions, { _, _, completion in
            completion(true)
        }, .global(qos: .userInteractive))
        sec_protocol_options_add_tls_application_protocol(secOptions, "fil")
        return options
    }
}
