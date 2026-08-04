import ComposableArchitecture
import Foundation

@Reducer
struct AppFeature {
    @ObservableState
    enum State: Equatable {
        case auth(AuthFeature.State)
        case main(MachinesFeature.State)

        init() {
            if TokenStorage.loadToken() != nil {
                self = .main(MachinesFeature.State())
            } else {
                self = .auth(AuthFeature.State())
            }
        }
    }

    enum Action {
        case auth(AuthFeature.Action)
        case main(MachinesFeature.Action)
        case openURL(URL)
    }

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .auth(.loginSucceeded):
                state = .main(MachinesFeature.State())
                return .none
            case .main(.logoutTapped):
                TokenStorage.clearToken()
                state = .auth(AuthFeature.State())
                return .none
            case .openURL(let url):
                guard url.scheme == "fil",
                      url.host == "session",
                      let sessionId = url.pathComponents.last,
                      !sessionId.isEmpty else { return .none }
                switch state {
                case .main:
                    return .send(.main(.openSession(sessionId)))
                case .auth:
                    return .none
                }
            default:
                return .none
            }
        }
        .ifCaseLet(\.auth, action: \.auth) {
            AuthFeature()
        }
        .ifCaseLet(\.main, action: \.main) {
            MachinesFeature()
        }
    }
}
