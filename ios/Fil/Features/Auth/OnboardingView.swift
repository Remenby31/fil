import SwiftUI

struct OnboardingView: View {
    @Binding var isPresented: Bool
    @State private var currentPage = 0

    private let pages: [(icon: String, title: String, subtitle: String)] = [
        (
            "desktopcomputer",
            "Your terminals follow you",
            "See every terminal session from your Mac, right on your iPhone."
        ),
        (
            "bell.badge",
            "Never miss a thing",
            "Get notified when builds finish, agents need input, or commands fail."
        ),
        (
            "lock.shield",
            "End-to-end encrypted",
            "Your terminal data is encrypted. The hub can't read it. Nobody can."
        ),
    ]

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            VStack(spacing: 0) {
                Spacer()

                // Page content
                TabView(selection: $currentPage) {
                    ForEach(0..<pages.count, id: \.self) { index in
                        onboardingPage(pages[index])
                            .tag(index)
                    }
                }
                .tabViewStyle(.page(indexDisplayMode: .never))
                .frame(height: 300)

                // Page dots
                HStack(spacing: 8) {
                    ForEach(0..<pages.count, id: \.self) { index in
                        Circle()
                            .fill(index == currentPage ? FilTheme.filGreen : FilTheme.cloud.opacity(0.15))
                            .frame(width: 7, height: 7)
                            .animation(.easeInOut(duration: 0.2), value: currentPage)
                    }
                }
                .padding(.top, 20)

                Spacer()

                // CTA
                Button {
                    if currentPage < pages.count - 1 {
                        withAnimation { currentPage += 1 }
                    } else {
                        isPresented = false
                    }
                } label: {
                    Text(currentPage < pages.count - 1 ? "Next" : "Get Started")
                        .font(.system(size: 16, weight: .semibold))
                        .foregroundStyle(FilTheme.void_)
                        .frame(maxWidth: .infinity)
                        .frame(height: 50)
                        .background(FilTheme.filGreen)
                        .clipShape(RoundedRectangle(cornerRadius: 14))
                }
                .padding(.horizontal, 32)

                if currentPage < pages.count - 1 {
                    Button {
                        isPresented = false
                    } label: {
                        Text("Skip")
                            .font(.system(size: 14))
                            .foregroundStyle(FilTheme.cloud.opacity(0.3))
                    }
                    .padding(.top, 12)
                }

                Spacer()
                    .frame(height: 40)
            }
        }
    }

    private func onboardingPage(_ page: (icon: String, title: String, subtitle: String)) -> some View {
        VStack(spacing: 20) {
            ZStack {
                Circle()
                    .fill(FilTheme.filGreen.opacity(0.08))
                    .frame(width: 88, height: 88)

                Image(systemName: page.icon)
                    .font(.system(size: 34))
                    .foregroundStyle(FilTheme.filGreen)
            }

            VStack(spacing: 10) {
                Text(page.title)
                    .font(.system(size: 22, weight: .semibold))
                    .foregroundStyle(FilTheme.cloud)

                Text(page.subtitle)
                    .font(.system(size: 15))
                    .foregroundStyle(FilTheme.cloud.opacity(0.45))
                    .multilineTextAlignment(.center)
                    .lineSpacing(3)
            }
        }
        .padding(.horizontal, 40)
    }
}
