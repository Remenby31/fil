import SwiftUI

struct OnboardingView: View {
    @Binding var isPresented: Bool
    var onComplete: () -> Void = {}

    @State private var currentPage = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    private let pages: [(icon: String, title: LocalizedStringKey, subtitle: LocalizedStringKey)] = [
        (
            "desktopcomputer",
            "Connect your Mac",
            "Install Fil on your Mac and active terminals appear securely on your devices."
        ),
        (
            "iphone.gen3",
            "Resume from anywhere",
            "Open a session, type, and keep working from iPhone or iPad."
        ),
        (
            "livephoto",
            "Follow intentionally",
            "Choose a terminal to show on the Lock Screen. Nothing is followed automatically."
        ),
    ]

    var body: some View {
        NavigationStack {
            VStack(spacing: 24) {
                TabView(selection: $currentPage) {
                    ForEach(pages.indices, id: \.self) { index in
                        page(pages[index])
                            .tag(index)
                    }
                }
                .tabViewStyle(.page(indexDisplayMode: .always))

                Button(currentPage < pages.count - 1 ? "Continue" : "Get Started") {
                    if currentPage < pages.count - 1 {
                        if reduceMotion {
                            currentPage += 1
                        } else {
                            withAnimation { currentPage += 1 }
                        }
                    } else {
                        complete()
                    }
                }
                .buttonStyle(.borderedProminent)
                .buttonBorderShape(.roundedRectangle(radius: 14))
                .tint(FilTheme.filGreen)
                .controlSize(.large)
                .frame(maxWidth: 520)
                .padding(.horizontal, 24)
                .padding(.bottom, 16)
            }
            .background(Color(uiColor: .systemBackground).ignoresSafeArea())
            .toolbar {
                if currentPage < pages.count - 1 {
                    ToolbarItem(placement: .topBarTrailing) {
                        Button("Skip") { complete() }
                    }
                }
            }
        }
    }

    private func page(
        _ page: (icon: String, title: LocalizedStringKey, subtitle: LocalizedStringKey)
    ) -> some View {
        ViewThatFits(in: .vertical) {
            VStack(spacing: 24) {
                Spacer()
                pageContent(page)
                Spacer()
            }

            ScrollView {
                pageContent(page)
                    .padding(.vertical, 28)
            }
        }
    }

    private func pageContent(
        _ page: (icon: String, title: LocalizedStringKey, subtitle: LocalizedStringKey)
    ) -> some View {
        VStack(spacing: 24) {
            Image(systemName: page.icon)
                .font(.system(size: 44, weight: .medium))
                .foregroundStyle(FilTheme.filGreen)
                .frame(width: 96, height: 96)
                .background(FilTheme.filGreen.opacity(0.1), in: Circle())
                .accessibilityHidden(true)

            VStack(spacing: 10) {
                Text(page.title)
                    .font(.title2.bold())
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)

                Text(page.subtitle)
                    .font(.body)
                    .foregroundStyle(.secondary)
                    .multilineTextAlignment(.center)
                    .lineLimit(nil)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: 520)
            .padding(.horizontal, 28)
        }
    }

    private func complete() {
        onComplete()
        isPresented = false
    }
}
