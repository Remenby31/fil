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
    }

    enum Action {
        case onAppear
        case onDisappear
        case connected
        case disconnected
        case dataReceived(Data)
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
                return .run { send in
                    // TODO: connect to hub WebSocket for this session's byte stream
                    await send(.connected)
                }

            case .onDisappear:
                return .none

            case .connected:
                state.isConnected = true
                state.showDisconnectedAlert = false
                return .none

            case .disconnected:
                state.isConnected = false
                state.showDisconnectedAlert = true
                return .none

            case .dataReceived:
                return .none

            case .inputSent:
                // TODO: send bytes to hub → daemon → PTY
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
                // TODO: implement session switching
                return .none
            }
        }
    }
}
