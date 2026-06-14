import AuthenticationServices
import ComposableArchitecture
import SwiftUI

struct AuthView: View {
    let store: StoreOf<AuthFeature>

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            VStack(spacing: 32) {
                Spacer()

                // Logo
                VStack(spacing: 8) {
                    Text("fil.")
                        .font(.system(size: 56, weight: .light))
                        .foregroundStyle(FilTheme.cloud)

                    Text("Your terminals, everywhere.")
                        .font(.system(size: 16, weight: .regular))
                        .foregroundStyle(FilTheme.cloud.opacity(0.5))
                }

                Spacer()

                // Auth buttons
                VStack(spacing: 14) {
                    SignInWithAppleButton(.signIn) { request in
                        request.requestedScopes = [.email, .fullName]
                    } onCompletion: { result in
                        store.send(.appleSignInCompleted(result))
                    }
                    .signInWithAppleButtonStyle(.white)
                    .frame(height: 52)
                    .clipShape(RoundedRectangle(cornerRadius: 12))

                    Button {
                        store.send(.signInWithGitHubTapped)
                    } label: {
                        HStack(spacing: 8) {
                            Image(systemName: "network")
                            Text("Sign in with GitHub")
                                .font(.system(size: 17, weight: .medium))
                        }
                        .foregroundStyle(FilTheme.cloud)
                        .frame(maxWidth: .infinity)
                        .frame(height: 52)
                        .background(FilTheme.surface)
                        .clipShape(RoundedRectangle(cornerRadius: 12))
                        .overlay(
                            RoundedRectangle(cornerRadius: 12)
                                .stroke(Color.white.opacity(0.1), lineWidth: 1)
                        )
                    }
                }

                if let error = store.errorMessage {
                    Text(error)
                        .font(.system(size: 14))
                        .foregroundStyle(FilTheme.error)
                        .multilineTextAlignment(.center)
                }

                if store.isLoading {
                    ProgressView()
                        .tint(FilTheme.filGreen)
                }

                Spacer()
                    .frame(height: 40)
            }
            .padding(.horizontal, 32)
        }
    }
}
