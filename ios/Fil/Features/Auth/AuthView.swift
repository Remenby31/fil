import AuthenticationServices
import ComposableArchitecture
import SwiftUI

struct AuthView: View {
    let store: StoreOf<AuthFeature>
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        ZStack {
            Color(uiColor: .systemBackground).ignoresSafeArea()

            if store.isCheckingToken {
                ProgressView("Checking your account…")
                    .tint(FilTheme.filGreen)
            } else {
                loginContent
            }
        }
        .onAppear { store.send(.onAppear) }
    }

    private var loginContent: some View {
        ScrollView {
            VStack(spacing: 32) {
                Spacer(minLength: 72)

                VStack(spacing: 20) {
                    FilBrandMark()
                        .frame(width: 132, height: 74)

                    VStack(spacing: 8) {
                        Text("Fil")
                            .font(.largeTitle.bold())

                        Text("Your terminals, wherever you are.")
                            .font(.body)
                            .foregroundStyle(.secondary)
                            .multilineTextAlignment(.center)
                    }
                }

                Spacer(minLength: 56)

                VStack(spacing: 12) {
                    SignInWithAppleButton(.signIn) { request in
                        request.requestedScopes = [.email, .fullName]
                    } onCompletion: { result in
                        store.send(.appleSignInCompleted(result))
                    }
                    .signInWithAppleButtonStyle(colorScheme == .dark ? .white : .black)
                    .frame(height: 52)
                    .clipShape(RoundedRectangle(cornerRadius: 13))
                    .disabled(store.isLoading)

                    Button {
                        store.send(.signInWithGitHubTapped)
                    } label: {
                        Label {
                            Text("Sign in with GitHub")
                                .font(.body.weight(.semibold))
                        } icon: {
                            Image("GitHubMark")
                                .resizable()
                                .renderingMode(.template)
                                .frame(width: 20, height: 20)
                                .foregroundStyle(.primary)
                        }
                        .frame(maxWidth: .infinity, minHeight: 50)
                    }
                    .buttonStyle(.bordered)
                    .buttonBorderShape(.roundedRectangle(radius: 13))
                    .disabled(store.isLoading)
                }

                if let error = store.errorMessage {
                    errorCallout(error)
                }

                if store.isLoading {
                    ProgressView("Signing in…")
                        .tint(FilTheme.filGreen)
                }

                Text("By continuing, you agree to the Privacy Policy.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)

                if let privacyURL = URL(string: "https://fil.remenby.fr/privacy") {
                    Link("Privacy Policy", destination: privacyURL)
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(FilTheme.filGreenText)
                        .frame(minHeight: 44)
                }

                Spacer(minLength: 32)
            }
            .frame(maxWidth: 520)
            .padding(.horizontal, 28)
            .frame(maxWidth: .infinity)
        }
    }

    private func errorCallout(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(FilTheme.error)
                .accessibilityHidden(true)

            Text(message)
                .font(.footnote)
                .foregroundStyle(FilTheme.error)
                .frame(maxWidth: .infinity, alignment: .leading)

            Button {
                store.send(.dismissError)
            } label: {
                Image(systemName: "xmark")
                    .frame(width: 44, height: 44)
            }
            .accessibilityLabel("Dismiss error")
        }
        .padding(.leading, 14)
        .background(FilTheme.error.opacity(0.1), in: RoundedRectangle(cornerRadius: 14))
    }
}
