import AuthenticationServices
import Foundation

enum GitHubAuthService {
    @MainActor
    static func authenticate(startURL: URL) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            let session = ASWebAuthenticationSession(
                url: startURL,
                callbackURLScheme: "fil"
            ) { callbackURL, error in
                if let error {
                    continuation.resume(throwing: error)
                    return
                }

                guard let url = callbackURL,
                      let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
                      let token = components.queryItems?.first(where: { $0.name == "token" })?.value else {
                    continuation.resume(throwing: AuthError.missingToken)
                    return
                }

                continuation.resume(returning: token)
            }

            session.prefersEphemeralWebBrowserSession = false
            session.start()
        }
    }

    enum AuthError: LocalizedError {
        case missingToken

        var errorDescription: String? {
            switch self {
            case .missingToken: "Authentication failed — no token received"
            }
        }
    }
}
