import SwiftUI
import ComposableArchitecture

struct AppView: View {
    let store: StoreOf<AppFeature>
    @State private var showOnboarding = false

    var body: some View {
        Group {
            switch store.state {
            case .auth:
                if let authStore = store.scope(state: \.auth, action: \.auth) {
                    AuthView(store: authStore)
                }
            case .main:
                if let mainStore = store.scope(state: \.main, action: \.main) {
                    MachinesView(store: mainStore)
                }
            }
        }
        .onOpenURL { url in
            // Deep link: fil://session/{sessionId}
            guard url.scheme == "fil",
                  url.host == "session",
                  let sessionId = url.pathComponents.last else { return }
            // TODO: navigate to specific session via store action
            _ = sessionId
        }
        .fullScreenCover(isPresented: $showOnboarding) {
            OnboardingView(isPresented: $showOnboarding)
        }
        .onAppear {
            let hasSeenOnboarding = UserDefaults.standard.bool(forKey: "hasSeenOnboarding")
            if !hasSeenOnboarding {
                showOnboarding = true
                UserDefaults.standard.set(true, forKey: "hasSeenOnboarding")
            }
        }
    }
}
