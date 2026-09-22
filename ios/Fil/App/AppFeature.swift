import ComposableArchitecture
import Foundation

@Reducer
struct AppFeature {
    @ObservableState
    enum State: Equatable {
        case auth(AuthFeature.State)
        case main(MachinesFeature.State)

        init() {
            // Bootstrap an existing account offline; Machines' first HTTP
            // snapshot verifies it and routes 401/403 back through login.
            if TokenStorage.loadToken() != nil {
                self = .main(MachinesFeature.State())
            } else {
                self = .auth(AuthFeature.State())
            }
        }
    }

    enum Action {
        case auth(AuthFeature.Action)
        case main(MachinesFeature.Action)
        case openURL(URL)
        case didBecomeActive
    }

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .auth(.loginSucceeded):
                state = .main(MachinesFeature.State())
                guard let url = FilSharedStore.takePendingActivityURL() else { return .none }
                // Start with an empty account snapshot. The normal open path
                // fetches ownership before resolving this exact session ID.
                return .send(.openURL(url))
            case .main(.logoutTapped):
                return requireLogin(&state, preserveSelection: false)
            case .main(.loginRequired):
                return requireLogin(&state, preserveSelection: true)
            case .didBecomeActive:
                // A warm foreground fires no .onAppear, so the Live Activity's
                // "Open terminal" button did nothing when the app was merely
                // backgrounded rather than cold-launched.
                var effects: [Effect<Action>] = []
                if case .main = state {
                    if let url = FilSharedStore.takePendingActivityURL() {
                        effects.append(.send(.openURL(url)))
                    }
                    effects.append(.send(.main(.didBecomeActive)))
                }
                return .merge(effects)

            case .openURL(let url):
                guard let scoped = Self.scopedSessionURL(url, hubURL: TokenStorage.loadHubUrl()),
                      let sessionId = scoped.pathComponents.last else { return .none }
                switch state {
                case .main:
                    return .send(.main(.openSession(sessionId)))
                case .auth:
                    FilSharedStore.savePendingActivityURL(scoped)
                    return .none
                }
            default:
                return .none
            }
        }
        .ifCaseLet(\.auth, action: \.auth) {
            AuthFeature()
        }
        .ifCaseLet(\.main, action: \.main) {
            MachinesFeature()
        }
    }

    private func requireLogin(_ state: inout State, preserveSelection: Bool) -> Effect<Action> {
        if preserveSelection, case .main(let main) = state,
           let sessionId = main.pendingSessionId ?? main.terminal?.session.id,
           let url = FilActivityURL.session(sessionId),
           let scoped = Self.scopedSessionURL(url, hubURL: TokenStorage.loadHubUrl()) {
            FilSharedStore.savePendingActivityURL(scoped)
        } else if !preserveSelection {
            // Explicit logout must not carry the old account's selection to
            // whoever signs in next. A new link while logged out is explicit.
            _ = FilSharedStore.takePendingActivityURL()
        }
        // These must finish before removing Machines and its effect lifetime.
        TerminalConnectionRegistry.shared.removeAllSessions()
        TokenStorage.clearToken()
        FilSharedStore.clearWidgetSnapshot()
        var auth = AuthFeature.State()
        auth.isCheckingToken = false
        if preserveSelection { auth.errorMessage = "Please sign in again to access this hub." }
        state = .auth(auth)
        // Root-owned cleanup survives removal of the child reducer.
        return .run { _ in
            if #available(iOS 16.2, *) { await FilActivityManager.shared.endAllImmediately() }
        }
    }

    /// Persist the existing shared deep link with its hub scope. Reauthentication
    /// must not reinterpret the same session ID on a newly configured hub.
    static func scopedSessionURL(_ url: URL, hubURL: String) -> URL? {
        guard var link = URLComponents(url: url, resolvingAgainstBaseURL: false),
              link.scheme == "fil", link.host == "session",
              link.user == nil, link.password == nil, link.port == nil,
              link.fragment == nil, url.pathComponents.count == 2,
              let sessionId = url.pathComponents.last, !sessionId.isEmpty,
              !sessionId.contains("/"), sessionId != ".", sessionId != "..",
              let hub = normalizedHub(hubURL) else { return nil }
        let scopes = (link.queryItems ?? []).filter { $0.name == "hub" }
        guard scopes.count <= 1 else { return nil }
        if let scope = scopes.first {
            guard let value = scope.value, normalizedHub(value) == hub else { return nil }
        }
        link.queryItems = [URLQueryItem(name: "hub", value: hub)]
        return link.url
    }

    private static func normalizedHub(_ value: String) -> String? {
        guard var url = URLComponents(string: value),
              let scheme = url.scheme?.lowercased(), ["https", "http"].contains(scheme),
              let host = url.host?.lowercased(), !host.isEmpty,
              url.user == nil, url.password == nil else { return nil }
        url.scheme = scheme
        url.host = host
        if (scheme == "https" && url.port == 443) || (scheme == "http" && url.port == 80) { url.port = nil }
        while url.path.hasSuffix("/") { url.path.removeLast() }
        url.query = nil
        url.fragment = nil
        return url.string
    }
}
