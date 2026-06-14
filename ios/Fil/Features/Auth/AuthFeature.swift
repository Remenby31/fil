import AuthenticationServices
import ComposableArchitecture
import Foundation

@Reducer
struct AuthFeature {
    @ObservableState
    struct State: Equatable {
        var isLoading = false
        var isCheckingToken = true
        var errorMessage: String?
    }

    enum Action {
        case onAppear
        case tokenCheckCompleted(Bool)
        case signInWithAppleTapped
        case signInWithGitHubTapped
        case appleSignInCompleted(Result<ASAuthorization, Error>)
        case hubAuthCompleted(Result<String, Error>)
        case loginSucceeded
        case loginFailed(String)
        case dismissError
    }

    @Dependency(\.hubClient) var hubClient

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear:
                state.isCheckingToken = true
                return .run { send in
                    if let token = TokenStorage.loadToken() {
                        let hubURL = TokenStorage.loadHubUrl()
                        let client = HubClient(baseURL: URL(string: hubURL)!)
                        do {
                            _ = try await client.health()
                            await send(.tokenCheckCompleted(true))
                        } catch {
                            await send(.tokenCheckCompleted(false))
                        }
                    } else {
                        await send(.tokenCheckCompleted(false))
                    }
                }

            case .tokenCheckCompleted(let valid):
                state.isCheckingToken = false
                if valid {
                    return .send(.loginSucceeded)
                }
                return .none

            case .signInWithAppleTapped:
                state.isLoading = true
                state.errorMessage = nil
                return .none

            case .signInWithGitHubTapped:
                state.isLoading = true
                state.errorMessage = nil
                let hubURL = TokenStorage.loadHubUrl()
                let authURL = URL(string: "\(hubURL)/auth/github/start")!
                return .run { send in
                    do {
                        let token = try await GitHubAuthService.authenticate(startURL: authURL)
                        await send(.hubAuthCompleted(.success(token)))
                    } catch {
                        await send(.hubAuthCompleted(.failure(error)))
                    }
                }

            case .appleSignInCompleted(.success(let authorization)):
                state.isLoading = true
                guard let credential = authorization.credential as? ASAuthorizationAppleIDCredential,
                      let identityToken = credential.identityToken,
                      let tokenString = String(data: identityToken, encoding: .utf8) else {
                    state.isLoading = false
                    state.errorMessage = "Failed to get Apple ID credential"
                    return .none
                }
                return .run { send in
                    do {
                        let hubURL = TokenStorage.loadHubUrl()
                        let client = HubClient(baseURL: URL(string: hubURL)!)
                        // TODO: implement Apple auth endpoint on hub
                        // For now, store a placeholder
                        TokenStorage.saveToken("apple-\(tokenString.prefix(20))")
                        await send(.loginSucceeded)
                    } catch {
                        await send(.hubAuthCompleted(.failure(error)))
                    }
                }

            case .appleSignInCompleted(.failure(let error)):
                state.isLoading = false
                let code = (error as? ASAuthorizationError)?.code
                if code == .canceled {
                    return .none
                }
                state.errorMessage = error.localizedDescription
                return .none

            case .hubAuthCompleted(.success(let token)):
                state.isLoading = false
                TokenStorage.saveToken(token)
                return .send(.loginSucceeded)

            case .hubAuthCompleted(.failure(let error)):
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

            case .dismissError:
                state.errorMessage = nil
                return .none
            }
        }
    }
}
