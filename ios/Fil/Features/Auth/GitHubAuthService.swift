import AuthenticationServices
import Foundation
import UIKit

final class GitHubAuthService: NSObject, ASWebAuthenticationPresentationContextProviding, @unchecked Sendable {
    private var session: ASWebAuthenticationSession?
    private static var activeInstance: GitHubAuthService?

    @MainActor
    static func authenticate(startURL: URL) async throws -> String {
        let service = GitHubAuthService()
        activeInstance = service

        defer { activeInstance = nil }

        return try await withCheckedThrowingContinuation { continuation in
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

            service.session = session
            session.presentationContextProvider = service
            session.prefersEphemeralWebBrowserSession = false
            session.start()
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }
            .flatMap(\.windows)
            .first(where: \.isKeyWindow) ?? ASPresentationAnchor()
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
