import ComposableArchitecture
import Foundation

@Reducer
struct MachinesFeature {
    @ObservableState
    struct State: Equatable {
        var machines: [Machine] = []
        var isLoading = false
        // A saved token permits an offline bootstrap, not a claim that the
        // hub has authenticated us. The first snapshot verifies that.
        var isConnected = false
        var errorMessage: String?
        var pendingSessionId: String?
        @Presents var terminal: TerminalFeature.State?
    }

    enum Action {
        case onAppear
        case onDisappear
        case didBecomeActive
        case refreshTapped
        case sessionsLoaded(Result<[Machine], Error>)
        case liveStatesReceived([DeviceState])
        case liveStreamFailed
        case openSession(String)
        case sessionTapped(Session)
        case terminal(PresentationAction<TerminalFeature.Action>)
        case logoutTapped
        case loginRequired
        case dismissError
    }

    @Dependency(\.hubClient) var hubClient
    @Dependency(\.sessionEvents) var sessionEvents

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear:
                state.isLoading = state.machines.isEmpty
                return .merge(loadMachines(), startEventStream())

            case .onDisappear:
                return .merge(.cancel(id: CancelID.events), .cancel(id: CancelID.snapshot))

            case .didBecomeActive:
                // iOS tears down URLSession websockets while suspended, and
                // the stream also finishes for good if the Keychain was locked
                // when it started. Restarting it here is what stops the list
                // from silently going stale after a background trip.
                state.errorMessage = nil
                return .merge(loadMachines(), startEventStream())

            case .refreshTapped:
                state.isLoading = state.machines.isEmpty
                state.errorMessage = nil
                // Refresh used to reload the list and clear the "Hub
                // unreachable" banner while leaving a dead event stream in
                // place, so the UI looked healthy and stopped updating.
                return .merge(loadMachines(), startEventStream())

            case .sessionsLoaded(.success(let machines)):
                state.isLoading = false
                state.isConnected = true
                state.errorMessage = nil
                state.machines = machines
                refreshTerminalMetadata(&state)
                let pendingEffect = resolvePendingSession(&state)
                persistWidgetSnapshot(machines)
                return pendingEffect

            case .sessionsLoaded(.failure(let error)):
                state.isLoading = false
                state.isConnected = false
                if Self.requiresLogin(error) {
                    state.errorMessage = nil
                    return .send(.loginRequired)
                }
                state.errorMessage = error.localizedDescription
                return .none

            case .liveStatesReceived(let states):
                state.isConnected = true
                state.errorMessage = nil
                let machines = Self.map(states)
                state.machines = machines
                refreshTerminalMetadata(&state)
                let pendingEffect = resolvePendingSession(&state)
                persistWidgetSnapshot(machines)
                return pendingEffect

            case .liveStreamFailed:
                state.isConnected = false
                return .none

            case .openSession(let sessionId):
                state.pendingSessionId = sessionId
                let wasResolved = state.machines
                    .flatMap(\.activeSessions)
                    .contains { $0.id == sessionId }
                // Mark the refresh as in flight BEFORE resolving. Otherwise
                // resolvePendingSession sees an unknown id with isLoading
                // false, gives up, and shows "no longer available" a beat
                // before the refresh it is about to trigger can answer --
                // which is what a Live Activity deep link hit every time.
                if !wasResolved {
                    state.isLoading = true
                }
                _ = resolvePendingSession(&state)
                return wasResolved ? .none : loadMachines()

            case .sessionTapped(let session):
                return .send(.openSession(session.id))

            case .terminal(.presented(.dismiss)):
                state.terminal = nil
                return .none

            case .terminal:
                return .none

            case .logoutTapped, .loginRequired:
                return .merge(
                    .cancel(id: CancelID.events),
                    .cancel(id: CancelID.snapshot)
                )

            case .dismissError:
                state.errorMessage = nil
                return .none
            }
        }
        .ifLet(\.$terminal, action: \.terminal) {
            TerminalFeature()
        }
    }

    private enum CancelID { case events, snapshot }

    private static func requiresLogin(_ error: Error) -> Bool {
        guard case HubError.httpError(let status) = error else { return false }
        return status == 401 || status == 403
    }

    /// `cancelInFlight` makes this safe to call repeatedly: a second start
    /// replaces the first rather than running two streams.
    private func startEventStream() -> Effect<Action> {
        .run { send in
            for await result in sessionEvents.updates() {
                guard !Task.isCancelled else { return }
                switch result {
                case .success(let states):
                    await send(.liveStatesReceived(states))
                case .failure(let error):
                    if Self.requiresLogin(error) {
                        await send(.loginRequired)
                        return
                    }
                    await send(.liveStreamFailed)
                }
            }
        }
        .cancellable(id: CancelID.events, cancelInFlight: true)
    }

    private func loadMachines() -> Effect<Action> {
        .run { send in
            do {
                let machines = try await hubClient.fetchMachines()
                guard !Task.isCancelled else { return }
                await send(.sessionsLoaded(.success(machines)))
            } catch {
                guard !Task.isCancelled else { return }
                await send(.sessionsLoaded(.failure(error)))
            }
        }
        .cancellable(id: CancelID.snapshot, cancelInFlight: true)
    }

    private func persistWidgetSnapshot(_ machines: [Machine]) {
        // Serialize this small cache write with logout's clear. An unscoped
        // async effect could otherwise restore the previous account's cache.
        let snapshots = machines.map { machine in
            FilWidgetMachineSnapshot(
                id: machine.id,
                name: machine.displayName,
                isConnected: machine.status == .online,
                sessions: machine.activeSessions.map { session in
                    FilWidgetSessionSnapshot(
                        id: session.id,
                        projectName: session.projectName,
                        processName: session.processName,
                        machineName: machine.displayName
                    )
                }
            )
        }
        FilSharedStore.saveWidgetSnapshot(.init(machines: snapshots))
    }

    private func refreshTerminalMetadata(_ state: inout State) {
        guard var terminal = state.terminal else { return }
        let contexts = state.machines.flatMap { machine in
            machine.activeSessions.map { TerminalSessionContext(session: $0, machineName: machine.displayName) }
        }
        if let current = contexts.first(where: {
            $0.id == terminal.session.id && $0.session.deviceId == terminal.session.deviceId
        }) {
            var session = current.session
            // Geometry belongs to the live phone view, not the list snapshot.
            session.cols = terminal.session.cols
            session.rows = terminal.session.rows
            terminal.session = session
            terminal.machineName = current.machineName
        }
        terminal.availableSessions = contexts
        terminal.otherSessionCount = max(0, contexts.count - (contexts.contains { $0.id == terminal.session.id } ? 1 : 0))
        // Preserve reducer-local connection/follow state and relay ownership.
        // Never select a different terminal when this ID disappears.
        state.terminal = terminal
    }

    private func resolvePendingSession(_ state: inout State) -> Effect<Action> {
        guard let sessionId = state.pendingSessionId else { return .none }
        for machine in state.machines {
            if let session = machine.activeSessions.first(where: { $0.id == sessionId }) {
                let availableSessions = state.machines.flatMap { machine in
                    machine.activeSessions.map {
                        TerminalSessionContext(session: $0, machineName: machine.displayName)
                    }
                }
                let activeCount = availableSessions.count
                state.pendingSessionId = nil
                state.terminal = TerminalFeature.State(
                    session: session,
                    machineName: machine.displayName,
                    otherSessionCount: max(0, activeCount - 1),
                    availableSessions: availableSessions
                )
                return .none
            }
        }
        guard !state.isLoading else { return .none }
        state.pendingSessionId = nil
        state.errorMessage = "This terminal is no longer available."
        return .none
    }

    static func map(_ deviceStates: [DeviceState]) -> [Machine] {
        deviceStates.map { deviceState in
            Machine(
                id: deviceState.deviceId,
                name: deviceState.deviceName ?? deviceState.deviceId,
                status: deviceState.connected ? .online : .offline,
                sessions: deviceState.sessions.map { dto in
                    Session(
                        id: dto.sessionId,
                        deviceId: dto.deviceId,
                        shell: dto.shell,
                        command: dto.command,
                        cwd: dto.cwd,
                        cols: dto.cols,
                        rows: dto.rows,
                        status: SessionStatus(rawValue: dto.status) ?? .offline,
                        createdAt: validSessionDate(dto.createdAt)
                    )
                }
                .sorted(by: Self.sessionSort)
            )
        }
        .sorted {
            let comparison = $0.displayName.localizedCaseInsensitiveCompare($1.displayName)
            return comparison == .orderedSame ? $0.id < $1.id : comparison == .orderedAscending
        }
    }

    private static func validSessionDate(_ value: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        let date = formatter.date(from: value) ?? {
            formatter.formatOptions.insert(.withFractionalSeconds)
            return formatter.date(from: value)
        }()
        guard let date,
              date.timeIntervalSince1970 > 86_400,
              date <= Date().addingTimeInterval(300) else {
            return nil
        }
        return date
    }

    private static func sessionSort(_ left: Session, _ right: Session) -> Bool {
        switch (left.createdAt, right.createdAt) {
        case let (.some(leftDate), .some(rightDate)) where leftDate != rightDate:
            return leftDate > rightDate
        case (.some, .none):
            return true
        case (.none, .some):
            return false
        default:
            return left.projectName.localizedCaseInsensitiveCompare(right.projectName) == .orderedAscending
        }
    }
}

struct HubClientDependency: Sendable {
    var fetchMachines: @Sendable () async throws -> [Machine]
}

extension HubClientDependency: DependencyKey {
    static let liveValue = HubClientDependency(
        fetchMachines: {
            guard let token = TokenStorage.loadToken() else {
                throw HubError.httpError(401)
            }
            return MachinesFeature.map(try await HubClient().listSessions(token: token))
        }
    )

    static let testValue = HubClientDependency(fetchMachines: { [] })
}

struct SessionEventsDependency: Sendable {
    var updates: @Sendable () -> AsyncStream<Result<[DeviceState], Error>>
}

extension SessionEventsDependency: DependencyKey {
    static let liveValue = SessionEventsDependency(
        updates: { SessionEventClient.updates() }
    )
    static let testValue = SessionEventsDependency(
        updates: { AsyncStream { $0.finish() } }
    )
}

extension DependencyValues {
    var hubClient: HubClientDependency {
        get { self[HubClientDependency.self] }
        set { self[HubClientDependency.self] = newValue }
    }

    var sessionEvents: SessionEventsDependency {
        get { self[SessionEventsDependency.self] }
        set { self[SessionEventsDependency.self] = newValue }
    }
}
