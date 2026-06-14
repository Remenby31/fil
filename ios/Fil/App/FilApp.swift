import SwiftUI
import ComposableArchitecture

@main
struct FilApp: App {
    static let store = Store(initialState: AppFeature.State()) {
        AppFeature()
    } withDependencies: {
        #if DEBUG
        $0.hubClient = .testValue
        #endif
    }

    var body: some Scene {
        WindowGroup {
            AppView(store: FilApp.store)
        }
    }
}
