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
                        TokenStorage.saveProvider("github")
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
                let email = credential.email
                let fullName = [credential.fullName?.givenName, credential.fullName?.familyName]
                    .compactMap { $0 }
                    .joined(separator: " ")
                let userIdentifier = credential.user

                return .run { send in
                    do {
                        let hubURL = TokenStorage.loadHubUrl()
                        let url = URL(string: "\(hubURL)/auth/apple/callback")!
                        var request = URLRequest(url: url)
                        request.httpMethod = "POST"
                        request.setValue("application/json", forHTTPHeaderField: "Content-Type")

                        let body: [String: Any?] = [
                            "identity_token": tokenString,
                            "user_id": userIdentifier,
                            "email": email,
                            "full_name": fullName.isEmpty ? nil : fullName,
                        ]
                        request.httpBody = try JSONSerialization.data(
                            withJSONObject: body.compactMapValues { $0 }
                        )

                        let (data, response) = try await URLSession.shared.data(for: request)
                        guard let http = response as? HTTPURLResponse,
                              (200...299).contains(http.statusCode) else {
                            throw HubError.httpError((response as? HTTPURLResponse)?.statusCode ?? 500)
                        }

                        struct AuthResponse: Decodable { let token: String }
                        let authResponse = try JSONDecoder().decode(AuthResponse.self, from: data)
                        TokenStorage.saveProvider("apple")
                        if let email { TokenStorage.saveEmail(email) }
                        await send(.hubAuthCompleted(.success(authResponse.token)))
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
