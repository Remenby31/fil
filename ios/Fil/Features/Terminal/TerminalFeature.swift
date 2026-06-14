import ComposableArchitecture
import Foundation

@Reducer
struct TerminalFeature {
    @ObservableState
    struct State: Equatable {
        var session: Session
        var isConnected = false
        var fontSize: CGFloat = 14
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
    }

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear:
                return .run { send in
                    // TODO: Connect to hub WebSocket for this session
                    await send(.connected)
                }

            case .onDisappear:
                return .none

            case .connected:
                state.isConnected = true
                return .none

            case .disconnected:
                state.isConnected = false
                return .none

            case .dataReceived:
                return .none

            case .inputSent:
                return .none

            case .dismiss:
                return .none

            case .fontSizeChanged(let size):
                state.fontSize = size
                return .none
            }
        }
    }
}
