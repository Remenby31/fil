import AuthenticationServices
import ComposableArchitecture
import Foundation

@Reducer
struct AuthFeature {
    @ObservableState
    struct State: Equatable {
        var isLoading = false
        var errorMessage: String?
    }

    enum Action {
        case signInWithAppleTapped
        case signInWithGitHubTapped
        case appleSignInCompleted(Result<ASAuthorization, Error>)
        case loginSucceeded
        case loginFailed(String)
    }

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .signInWithAppleTapped:
                state.isLoading = true
                state.errorMessage = nil
                return .none

            case .signInWithGitHubTapped:
                state.isLoading = true
                state.errorMessage = nil
                return .none

            case .appleSignInCompleted(.success):
                state.isLoading = false
                // TODO: Exchange Apple credential for hub JWT
                TokenStorage.saveToken("placeholder-token")
                return .send(.loginSucceeded)

            case .appleSignInCompleted(.failure(let error)):
                state.isLoading = false
                state.errorMessage = error.localizedDescription
                return .none

            case .loginSucceeded:
                state.isLoading = false
                return .none

            case .loginFailed(let message):
                state.isLoading = false
                state.errorMessage = message
                return .none
            }
        }
    }
}
