import SwiftUI
import ComposableArchitecture
import UIKit

@main
struct FilApp: App {
    static let store: StoreOf<AppFeature> = {
        #if DEBUG && targetEnvironment(simulator)
        SimulatorLaunchConfiguration.install()
        #endif
        return Store(initialState: AppFeature.State()) {
            AppFeature()
        }
    }()

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
                if case .main(let main) = FilApp.store.state, main.terminal != nil {
                    FilApp.store.send(.main(.terminal(.presented(.refreshFollowingStatus))))
                }
            default:
                break
            }
        }
    }
}

#if DEBUG && targetEnvironment(simulator)
/// Live QA only. Both the call site and this implementation are compiled out
/// of physical-device builds (including Debug) and every Release build.
/// Credentials are never logged; all traffic still uses the real hub/client.
struct SimulatorLaunchConfiguration {
    let hubURL: String
    let token: String
    let sessionURL: URL?

    init?(environment: [String: String]) {
        guard let hubURL = environment["FIL_TEST_HUB_URL"],
              let hub = URLComponents(string: hubURL),
              hub.scheme?.lowercased() == "https",
              let host = hub.host, !host.isEmpty,
              hub.user == nil, hub.password == nil,
              hub.query == nil, hub.fragment == nil, hub.url != nil,
              let token = environment["FIL_TEST_TOKEN"], !token.isEmpty,
              token.rangeOfCharacter(from: .whitespacesAndNewlines) == nil else { return nil }

        var sessionURL: URL?
        if let sessionId = environment["FIL_TEST_SESSION_ID"] {
            // A single explicit session identifier; never pick the first
            // session or allow a path/query to select a different terminal.
            let allowed = CharacterSet.alphanumerics.union(CharacterSet(charactersIn: "-_"))
            guard !sessionId.isEmpty,
                  sessionId.rangeOfCharacter(from: allowed.inverted) == nil,
                  let url = FilActivityURL.session(sessionId) else { return nil }
            sessionURL = url
        }
        self.hubURL = hubURL
        self.token = token
        self.sessionURL = sessionURL
    }

    static func install() {
        guard let configuration = Self(environment: ProcessInfo.processInfo.environment) else { return }
        // Run before AppFeature.State chooses its authenticated root.
        TokenStorage.saveHubUrl(configuration.hubURL)
        TokenStorage.saveToken(configuration.token)
        UserDefaults.standard.set(true, forKey: "hasSeenOnboarding")
        // A previous Live Activity link must not select an unrelated terminal
        // during QA. With no scratch ID this launch stays on Machines.
        _ = FilSharedStore.takePendingActivityURL()
        if let url = configuration.sessionURL {
            FilSharedStore.savePendingActivityURL(url)
        }
    }
}
#endif

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
    @MainActor
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

        TerminalConnectionRegistry.shared.applicationDidEnterBackground {
            Task { @MainActor in
                guard box.id != .invalid else { return }
                app.endBackgroundTask(box.id)
                box.id = .invalid
            }
        }
    }

    static func willEnterForeground() {
        TerminalConnectionRegistry.shared.applicationWillEnterForeground()
    }
}
