import SwiftUI
import ComposableArchitecture

@main
struct FilApp: App {
    static let store = Store(initialState: AppFeature.State()) {
        AppFeature()
    }

    var body: some Scene {
        WindowGroup {
            AppView(store: FilApp.store)
        }
    }
}
