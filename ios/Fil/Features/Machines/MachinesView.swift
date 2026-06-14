import ComposableArchitecture
import SwiftUI

struct MachinesView: View {
    @Bindable var store: StoreOf<MachinesFeature>

    var body: some View {
        NavigationStack {
            ZStack {
                FilTheme.void_.ignoresSafeArea()

                if store.machines.isEmpty && !store.isLoading {
                    emptyState
                } else {
                    machinesList
                }
            }
            .navigationTitle("Machines")
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        store.send(.logoutTapped)
                    } label: {
                        Image(systemName: "gearshape")
                            .foregroundStyle(FilTheme.cloud.opacity(0.6))
                    }
                }
            }
            .refreshable {
                store.send(.refreshTapped)
            }
            .onAppear {
                store.send(.onAppear)
            }
            .fullScreenCover(item: $store.scope(state: \.terminal, action: \.terminal)) { terminalStore in
                TerminalSessionView(store: terminalStore)
            }
        }
        .preferredColorScheme(.dark)
    }

    private var machinesList: some View {
        ScrollView {
            LazyVStack(spacing: 12) {
                ForEach(store.machines) { machine in
                    MachineCard(machine: machine) { session in
                        store.send(.sessionTapped(session))
                    }
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
        }
    }

    private var emptyState: some View {
        VStack(spacing: 16) {
            Image(systemName: "desktopcomputer")
                .font(.system(size: 48))
                .foregroundStyle(FilTheme.cloud.opacity(0.2))

            Text("No machines connected")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(FilTheme.cloud.opacity(0.5))

            Text("Install fil on your Mac and run\nfil setup to get started.")
                .font(.system(size: 14))
                .foregroundStyle(FilTheme.cloud.opacity(0.3))
                .multilineTextAlignment(.center)

            if let error = store.errorMessage {
                Text(error)
                    .font(.system(size: 13))
                    .foregroundStyle(FilTheme.error)
                    .padding(.top, 8)
            }
        }
    }
}

// MARK: - Machine Card

struct MachineCard: View {
    let machine: Machine
    let onSessionTap: (Session) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            // Machine header
            HStack(spacing: 8) {
                Circle()
                    .fill(statusColor)
                    .frame(width: 8, height: 8)

                Text(machine.name)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(FilTheme.cloud)

                Spacer()

                if machine.status == .offline {
                    Text("offline")
                        .font(.system(size: 12))
                        .foregroundStyle(FilTheme.cloud.opacity(0.3))
                }
            }

            // Sessions
            if machine.sessions.isEmpty && machine.status != .offline {
                Text("No active sessions")
                    .font(.system(size: 13))
                    .foregroundStyle(FilTheme.cloud.opacity(0.3))
            } else {
                ForEach(machine.sessions) { session in
                    Button {
                        onSessionTap(session)
                    } label: {
                        SessionRow(session: session)
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .padding(16)
        .background(FilTheme.surface)
        .clipShape(RoundedRectangle(cornerRadius: 14))
        .overlay(
            RoundedRectangle(cornerRadius: 14)
                .stroke(Color.white.opacity(0.04), lineWidth: 1)
        )
    }

    private var statusColor: Color {
        switch machine.status {
        case .online: FilTheme.online
        case .unreachable: FilTheme.unreachable
        case .offline: FilTheme.offline
        }
    }
}

// MARK: - Session Row

struct SessionRow: View {
    let session: Session

    var body: some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(session.shell)
                    .font(.system(size: 14, weight: .medium, design: .monospaced))
                    .foregroundStyle(FilTheme.cloud)

                Text(session.cwd)
                    .font(.system(size: 12))
                    .foregroundStyle(FilTheme.cloud.opacity(0.4))
            }

            Spacer()

            Text(session.duration)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(FilTheme.cloud.opacity(0.3))

            Image(systemName: "chevron.right")
                .font(.system(size: 11))
                .foregroundStyle(FilTheme.cloud.opacity(0.2))
        }
        .padding(10)
        .background(FilTheme.elevated)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}
