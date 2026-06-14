import ComposableArchitecture
import Foundation

@Reducer
struct MachinesFeature {
    @ObservableState
    struct State: Equatable {
        var machines: [Machine] = []
        var isLoading = false
        var errorMessage: String?
        var selectedSession: Session?
        @Presents var terminal: TerminalFeature.State?
    }

    enum Action {
        case onAppear
        case refreshTapped
        case sessionsLoaded(Result<[Machine], Error>)
        case sessionTapped(Session)
        case terminal(PresentationAction<TerminalFeature.Action>)
        case logoutTapped
    }

    @Dependency(\.hubClient) var hubClient

    var body: some ReducerOf<Self> {
        Reduce { state, action in
            switch action {
            case .onAppear, .refreshTapped:
                state.isLoading = true
                state.errorMessage = nil
                return .run { send in
                    do {
                        let machines = try await hubClient.fetchMachines()
                        await send(.sessionsLoaded(.success(machines)))
                    } catch {
                        await send(.sessionsLoaded(.failure(error)))
                    }
                }

            case .sessionsLoaded(.success(let machines)):
                state.isLoading = false
                state.machines = machines
                return .none

            case .sessionsLoaded(.failure(let error)):
                state.isLoading = false
                state.errorMessage = error.localizedDescription
                return .none

            case .sessionTapped(let session):
                state.selectedSession = session
                state.terminal = TerminalFeature.State(session: session)
                return .none

            case .terminal:
                return .none

            case .logoutTapped:
                return .none
            }
        }
        .ifLet(\.$terminal, action: \.terminal) {
            TerminalFeature()
        }
    }
}

// MARK: - Hub Client Dependency

struct HubClientDependency: Sendable {
    var fetchMachines: @Sendable () async throws -> [Machine]
}

extension HubClientDependency: DependencyKey {
    static let liveValue = HubClientDependency(
        fetchMachines: {
            guard let token = TokenStorage.loadToken() else {
                throw HubError.httpError(401)
            }
            let client = HubClient()
            let deviceStates = try await client.listSessions(token: token)

            var machineMap: [String: Machine] = [:]
            for deviceState in deviceStates {
                let sessions = deviceState.sessions.map { dto in
                    Session(
                        id: dto.sessionId,
                        deviceId: dto.deviceId,
                        shell: dto.shell,
                        cwd: dto.cwd,
                        cols: dto.cols,
                        rows: dto.rows,
                        status: SessionStatus(rawValue: dto.status) ?? .offline,
                        createdAt: ISO8601DateFormatter().date(from: dto.createdAt) ?? Date()
                    )
                }
                let machine = Machine(
                    id: deviceState.deviceId,
                    name: deviceState.deviceId,
                    status: deviceState.connected ? .online : .offline,
                    sessions: sessions
                )
                machineMap[deviceState.deviceId] = machine
            }
            return Array(machineMap.values).sorted { $0.name < $1.name }
        }
    )

    static let testValue = HubClientDependency(
        fetchMachines: {
            [
                Machine(
                    id: "mac-mini",
                    name: "Mac mini",
                    status: .online,
                    sessions: [
                        Session(id: "1", deviceId: "mac-mini", shell: "zsh", cwd: "~/projects/fil", cols: 80, rows: 24, status: .online, createdAt: Date().addingTimeInterval(-180)),
                        Session(id: "2", deviceId: "mac-mini", shell: "node", cwd: "~/projects/web", cols: 80, rows: 24, status: .online, createdAt: Date().addingTimeInterval(-2700)),
                    ]
                ),
                Machine(
                    id: "macbook",
                    name: "MacBook Pro",
                    status: .offline,
                    sessions: []
                ),
            ]
        }
    )
}

extension DependencyValues {
    var hubClient: HubClientDependency {
        get { self[HubClientDependency.self] }
        set { self[HubClientDependency.self] = newValue }
    }
}
