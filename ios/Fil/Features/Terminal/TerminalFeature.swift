import ComposableArchitecture
import Foundation

@Reducer
struct TerminalFeature {
    @ObservableState
    struct State: Equatable {
        var session: Session
        var isConnected = false
        var fontSize: CGFloat = 14
        var latencyMs: Int?
        var showDisconnectedAlert = false
        var terminalData: [UInt8] = []
    }

    enum Action: Equatable {
        case onAppear
        case onDisappear
        case connected
        case disconnected
        case dataReceived([UInt8])
        case inputSent(Data)
        case dismiss
        case fontSizeChanged(CGFloat)
        case latencyUpdated(Int)
        case reconnectTapped
        case nextSession
        case previousSession
    }

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear:
                let sessionId = state.session.id
                // QUIC host: derive from hub URL (fil.remenby.fr → quic.fil.remenby.fr)
                let httpHost = TokenStorage.loadHubUrl()
                    .replacingOccurrences(of: "https://", with: "")
                    .replacingOccurrences(of: "http://", with: "")
                    .components(separatedBy: ":").first ?? "localhost"
                let hubHost = "quic.\(httpHost)"

                return .run { send in
                    let client = QUICTerminalClient(hubHost: hubHost)

                    client.onConnected = {
                        Task { await send(.connected) }
                    }
                    client.onDisconnected = {
                        Task { await send(.disconnected) }
                    }
                    client.onDataReceived = { data in
                        Task { await send(.dataReceived([UInt8](data))) }
                    }

                    client.connect(sessionId: sessionId)

                    // Keep the client alive until cancelled
                    try? await Task.sleep(for: .seconds(86400))
                    client.disconnect()
                }

            case .onDisappear:
                return .cancel(id: CancelID.quic)

            case .connected:
                state.isConnected = true
                state.showDisconnectedAlert = false
                return .none

            case .disconnected:
                state.isConnected = false
                state.showDisconnectedAlert = true
                return .none

            case .dataReceived(let bytes):
                state.terminalData = bytes
                return .none

            case .inputSent(let data):
                // TODO: send via QUIC client
                return .none

            case .dismiss:
                return .none

            case .fontSizeChanged(let size):
                state.fontSize = max(10, min(24, size))
                return .none

            case .latencyUpdated(let ms):
                state.latencyMs = ms
                return .none

            case .reconnectTapped:
                state.showDisconnectedAlert = false
                return .send(.onAppear)

            case .nextSession, .previousSession:
                return .none
            }
        }
    }

    private enum CancelID { case quic }
}
