import AuthenticationServices
import ComposableArchitecture
import SwiftUI

struct AuthView: View {
    let store: StoreOf<AuthFeature>

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            if store.isCheckingToken {
                splashView
            } else {
                loginView
            }
        }
        .onAppear {
            store.send(.onAppear)
        }
    }

    private var splashView: some View {
        VStack(spacing: 16) {
            Text("fil.")
                .font(.system(size: 56, weight: .light))
                .foregroundStyle(FilTheme.cloud)

            ProgressView()
                .tint(FilTheme.filGreen)
                .scaleEffect(0.8)
        }
    }

    private var loginView: some View {
        VStack(spacing: 32) {
            Spacer()

            VStack(spacing: 12) {
                Text("fil.")
                    .font(.system(size: 56, weight: .light))
                    .foregroundStyle(FilTheme.cloud)

                Text("Your terminals, everywhere.")
                    .font(.system(size: 16, weight: .regular))
                    .foregroundStyle(FilTheme.cloud.opacity(0.5))
            }

            Spacer()

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
                .disabled(store.isLoading)
            }

            if let error = store.errorMessage {
                HStack(spacing: 8) {
                    Image(systemName: "exclamationmark.triangle.fill")
                        .foregroundStyle(FilTheme.error)
                        .font(.system(size: 14))

                    Text(error)
                        .font(.system(size: 13))
                        .foregroundStyle(FilTheme.error)
                        .lineLimit(2)

                    Spacer()

                    Button {
                        store.send(.dismissError)
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 12))
                            .foregroundStyle(FilTheme.cloud.opacity(0.4))
                    }
                }
                .padding(12)
                .background(FilTheme.error.opacity(0.1))
                .clipShape(RoundedRectangle(cornerRadius: 10))
            }

            if store.isLoading {
                ProgressView()
                    .tint(FilTheme.filGreen)
            }

            Spacer()
                .frame(height: 40)
        }
        .padding(.horizontal, 32)
        .animation(.easeInOut(duration: 0.2), value: store.errorMessage)
        .animation(.easeInOut(duration: 0.2), value: store.isLoading)
    }
}
