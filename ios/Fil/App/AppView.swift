import SwiftUI
import ComposableArchitecture

struct AppView: View {
    let store: StoreOf<AppFeature>

    var body: some View {
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
}
