import SwiftUI
import ComposableArchitecture
import UIKit

@main
struct FilApp: App {
    static let store = Store(initialState: AppFeature.State()) {
        AppFeature()
    }

    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            AppView(store: FilApp.store)
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .background:
                // Deliberately NOT .inactive. Peeking at the app switcher or
                // pulling down Notification Centre goes
                // .active -> .inactive -> .active; releasing the Mac's PTY on
                // that would churn the user's shell for a glance.
                AppLifecycle.didEnterBackground()
            case .active:
                AppLifecycle.willEnterForeground()
                FilApp.store.send(.didBecomeActive)
            default:
                break
            }
        }
    }
}

/// Bridges scene phase to the connection layer.
///
/// Before this existed there was no lifecycle handling anywhere in the app:
/// no scenePhase, no UIBackgroundModes, no background task. The app could not
/// tell it had been suspended, so it never detached cleanly and never
/// reconnected on return -- the user had to tap a button.
@MainActor
enum AppLifecycle {
    /// Reference box: the expiration handler has to be able to end the task
    /// without the identifier crossing an isolation boundary by value.
    private final class BackgroundTaskBox {
        var id: UIBackgroundTaskIdentifier = .invalid
    }

    static func didEnterBackground() {
        // iOS suspends the process a few seconds after this callback. Ask for
        // a little guaranteed runtime so the detach frame actually reaches the
        // hub, which releases the Mac's PTY size *now* rather than 17s later
        // when the idle timeout finally reaps us. No UIBackgroundModes entry
        // and no App Review implications: this is the ordinary
        // finish-what-you-started allowance.
        let app = UIApplication.shared
        let box = BackgroundTaskBox()
        box.id = app.beginBackgroundTask(withName: "sh.fil.detach") {
            MainActor.assumeIsolated {
                guard box.id != .invalid else { return }
                app.endBackgroundTask(box.id)
                box.id = .invalid
            }
        }

        TerminalConnectionRegistry.shared.applicationDidEnterBackground()

        if box.id != .invalid {
            app.endBackgroundTask(box.id)
            box.id = .invalid
        }
    }

    static func willEnterForeground() {
        TerminalConnectionRegistry.shared.applicationWillEnterForeground()
    }
}
