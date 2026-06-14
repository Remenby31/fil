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
            TypewriterText(fullText: "fil.sh")

            ProgressView()
                .tint(FilTheme.filGreen)
                .scaleEffect(0.8)
        }
    }

    private var loginView: some View {
        VStack(spacing: 32) {
            Spacer()

            VStack(spacing: 12) {
                TypewriterText(fullText: "fil.sh")

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
                        Image("GitHubMark")
                            .resizable()
                            .frame(width: 20, height: 20)
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

// MARK: - Typewriter Effect

struct TypewriterText: View {
    let fullText: String
    private let suffix = ".sh"
    @State private var displayedSuffix = ""
    @State private var charIndex = 0
    @State private var cursorVisible = true

    private let typeSpeed: Double = 0.12
    private let deleteSpeed: Double = 0.08
    private let pauseBeforeDelete: Double = 2.5
    private let pauseBeforeType: Double = 0.8

    var body: some View {
        HStack(spacing: 0) {
            Text("fil")
                .font(.system(size: 56, weight: .light))
                .foregroundStyle(FilTheme.cloud)

            Text(displayedSuffix)
                .font(.system(size: 56, weight: .light))
                .foregroundStyle(FilTheme.filGreen)

            Rectangle()
                .fill(FilTheme.filGreen)
                .frame(width: 2, height: 46)
                .opacity(cursorVisible ? 1.0 : 0.0)
        }
        .onAppear { startAnimation() }
    }

    private func startAnimation() {
        Timer.scheduledTimer(withTimeInterval: 0.53, repeats: true) { _ in
            cursorVisible.toggle()
        }
        typeNextChar()
    }

    private func typeNextChar() {
        guard charIndex < suffix.count else {
            DispatchQueue.main.asyncAfter(deadline: .now() + pauseBeforeDelete) {
                deleteNextChar()
            }
            return
        }

        let index = suffix.index(suffix.startIndex, offsetBy: charIndex)
        displayedSuffix.append(suffix[index])
        charIndex += 1

        DispatchQueue.main.asyncAfter(deadline: .now() + typeSpeed) {
            typeNextChar()
        }
    }

    private func deleteNextChar() {
        guard !displayedSuffix.isEmpty else {
            charIndex = 0
            DispatchQueue.main.asyncAfter(deadline: .now() + pauseBeforeType) {
                typeNextChar()
            }
            return
        }

        displayedSuffix.removeLast()

        DispatchQueue.main.asyncAfter(deadline: .now() + deleteSpeed) {
            deleteNextChar()
        }
    }
}

// MARK: - GitHub Icon

struct GitHubIcon: View {
    let size: CGFloat

    var body: some View {
        Canvas { context, canvasSize in
            let s = min(canvasSize.width, canvasSize.height)
            let center = CGPoint(x: s / 2, y: s / 2)
            let r = s * 0.45

            // Main circle (head)
            let headPath = Path(ellipseIn: CGRect(
                x: center.x - r, y: center.y - r,
                width: r * 2, height: r * 2
            ))
            context.fill(headPath, with: .color(.white))

            // Inner cutout to create octocat silhouette
            let innerR = r * 0.55
            let innerPath = Path(ellipseIn: CGRect(
                x: center.x - innerR, y: center.y - innerR * 0.7,
                width: innerR * 2, height: innerR * 1.6
            ))
            context.fill(innerPath, with: .color(FilTheme.surface))

            // Eyes
            let eyeR = r * 0.12
            let eyeY = center.y - r * 0.05
            let leftEye = Path(ellipseIn: CGRect(
                x: center.x - r * 0.25 - eyeR, y: eyeY - eyeR,
                width: eyeR * 2, height: eyeR * 2
            ))
            let rightEye = Path(ellipseIn: CGRect(
                x: center.x + r * 0.25 - eyeR, y: eyeY - eyeR,
                width: eyeR * 2, height: eyeR * 2
            ))
            context.fill(leftEye, with: .color(.white))
            context.fill(rightEye, with: .color(.white))
        }
        .frame(width: size, height: size)
    }
}
