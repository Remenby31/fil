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
            store.send(.openURL(url))
        }
        .fullScreenCover(isPresented: $showOnboarding) {
            OnboardingView(isPresented: $showOnboarding) {
                UserDefaults.standard.set(true, forKey: "hasSeenOnboarding")
            }
        }
        .onAppear {
            if let url = FilSharedStore.takePendingActivityURL() {
                store.send(.openURL(url))
            }
            presentOnboardingIfNeeded()
        }
        .onChange(of: isShowingMainApp) { _, isMain in
            if isMain { presentOnboardingIfNeeded() }
        }
        .task {
            await migrateLegacyLiveActivitiesIfNeeded()
        }
    }

    private var isShowingMainApp: Bool {
        if case .main = store.state { return true }
        return false
    }

    private func presentOnboardingIfNeeded() {
        guard isShowingMainApp,
              !UserDefaults.standard.bool(forKey: "hasSeenOnboarding") else {
            return
        }
        showOnboarding = true
    }

    private func migrateLegacyLiveActivitiesIfNeeded() async {
        let migrationKey = "didMigrateToExplicitLiveActivities"
        guard !UserDefaults.standard.bool(forKey: migrationKey) else { return }
        if #available(iOS 16.2, *) {
            await FilActivityManager.shared.endAllImmediately()
        }
        UserDefaults.standard.set(true, forKey: migrationKey)
    }
}
