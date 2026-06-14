import ComposableArchitecture
import SwiftUI

struct MachinesView: View {
    @Bindable var store: StoreOf<MachinesFeature>
    @State private var showSettings = false

    var body: some View {
        NavigationStack {
            ZStack {
                FilTheme.void_.ignoresSafeArea()

                VStack(spacing: 0) {
                    if !store.isConnected {
                        connectionBanner
                    }

                    if store.machines.isEmpty && !store.isLoading {
                        emptyState
                    } else {
                        machinesList
                    }
                }

                if store.isLoading && store.machines.isEmpty {
                    loadingSkeleton
                }
            }
            .navigationTitle("Machines")
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showSettings = true
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
            .sheet(isPresented: $showSettings) {
                SettingsView {
                    showSettings = false
                    store.send(.logoutTapped)
                }
            }
        }
        .preferredColorScheme(.dark)
    }

    // MARK: - Connection Banner

    private var connectionBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "wifi.slash")
                .font(.system(size: 12))
            Text("Hub unreachable")
                .font(.system(size: 13, weight: .medium))
            Spacer()
            Button("Retry") {
                store.send(.refreshTapped)
            }
            .font(.system(size: 13, weight: .medium))
            .foregroundStyle(FilTheme.filGreen)
        }
        .foregroundStyle(FilTheme.warning)
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(FilTheme.warning.opacity(0.1))
    }

    // MARK: - Machines List

    private var machinesList: some View {
        ScrollView {
            LazyVStack(spacing: 12) {
                ForEach(store.machines) { machine in
                    MachineCard(machine: machine) { session in
                        store.send(.sessionTapped(session))
                    }
                    .transition(.asymmetric(
                        insertion: .move(edge: .top).combined(with: .opacity),
                        removal: .opacity
                    ))
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 8)
            .padding(.bottom, 20)
            .animation(.easeInOut(duration: 0.3), value: store.machines)

            if let error = store.errorMessage {
                errorBanner(error)
                    .padding(.horizontal, 16)
            }
        }
    }

    // MARK: - Loading Skeleton

    private var loadingSkeleton: some View {
        VStack(spacing: 12) {
            ForEach(0..<3, id: \.self) { _ in
                RoundedRectangle(cornerRadius: 14)
                    .fill(FilTheme.surface)
                    .frame(height: 120)
                    .shimmer()
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 80)
    }

    // MARK: - Empty State

    private var emptyState: some View {
        VStack(spacing: 20) {
            Spacer()

            ZStack {
                Circle()
                    .fill(FilTheme.surface)
                    .frame(width: 80, height: 80)

                Image(systemName: "desktopcomputer")
                    .font(.system(size: 32))
                    .foregroundStyle(FilTheme.cloud.opacity(0.3))
            }

            VStack(spacing: 8) {
                Text("No machines connected")
                    .font(.system(size: 20, weight: .semibold))
                    .foregroundStyle(FilTheme.cloud)

                Text("Install fil on your Mac to get started")
                    .font(.system(size: 15))
                    .foregroundStyle(FilTheme.cloud.opacity(0.4))
            }

            VStack(alignment: .leading, spacing: 8) {
                codeStep("1", "brew install fil")
                codeStep("2", "fil setup")
                codeStep("3", "Restart your terminal")
            }
            .padding(20)
            .background(FilTheme.depth)
            .clipShape(RoundedRectangle(cornerRadius: 12))
            .padding(.horizontal, 40)

            Spacer()
        }
    }

    private func codeStep(_ num: String, _ code: String) -> some View {
        HStack(spacing: 12) {
            Text(num)
                .font(.system(size: 12, weight: .bold, design: .monospaced))
                .foregroundStyle(FilTheme.void_)
                .frame(width: 22, height: 22)
                .background(FilTheme.filGreen)
                .clipShape(Circle())

            Text(code)
                .font(.system(size: 14, design: .monospaced))
                .foregroundStyle(FilTheme.filGreen)
        }
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(FilTheme.error)
                .font(.system(size: 13))

            Text(message)
                .font(.system(size: 13))
                .foregroundStyle(FilTheme.error.opacity(0.8))

            Spacer()

            Button { store.send(.dismissError) } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 11))
                    .foregroundStyle(FilTheme.cloud.opacity(0.3))
            }
        }
        .padding(12)
        .background(FilTheme.error.opacity(0.08))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }
}

// MARK: - Machine Card

struct MachineCard: View {
    let machine: Machine
    let onSessionTap: (Session) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                PulsingDot(color: statusColor)

                Text(machine.name)
                    .font(.system(size: 15, weight: .semibold))
                    .foregroundStyle(FilTheme.cloud)

                Spacer()

                if machine.status == .offline {
                    Text("offline")
                        .font(.system(size: 12))
                        .foregroundStyle(FilTheme.cloud.opacity(0.3))
                } else if !machine.sessions.isEmpty {
                    Text("\(machine.sessions.count)")
                        .font(.system(size: 12, weight: .medium, design: .monospaced))
                        .foregroundStyle(FilTheme.filGreen)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 3)
                        .background(FilTheme.filGreen.opacity(0.1))
                        .clipShape(Capsule())
                }
            }

            if machine.sessions.isEmpty && machine.status != .offline {
                Text("No active sessions")
                    .font(.system(size: 13))
                    .foregroundStyle(FilTheme.cloud.opacity(0.3))
                    .padding(.vertical, 4)
            } else {
                ForEach(machine.sessions) { session in
                    Button { onSessionTap(session) } label: {
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
            VStack(alignment: .leading, spacing: 3) {
                Text(session.shell)
                    .font(.system(size: 14, weight: .medium, design: .monospaced))
                    .foregroundStyle(FilTheme.cloud)

                Text(session.cwd)
                    .font(.system(size: 12))
                    .foregroundStyle(FilTheme.cloud.opacity(0.4))
                    .lineLimit(1)
            }

            Spacer()

            Text(session.duration)
                .font(.system(size: 12, design: .monospaced))
                .foregroundStyle(FilTheme.cloud.opacity(0.3))

            Image(systemName: "chevron.right")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(FilTheme.cloud.opacity(0.15))
        }
        .padding(10)
        .background(FilTheme.elevated)
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}

// MARK: - Pulsing Dot

struct PulsingDot: View {
    let color: Color
    @State private var isPulsing = false

    var body: some View {
        ZStack {
            Circle()
                .fill(color.opacity(0.3))
                .frame(width: 14, height: 14)
                .scaleEffect(isPulsing ? 1.3 : 1.0)
                .opacity(isPulsing ? 0 : 0.5)

            Circle()
                .fill(color)
                .frame(width: 8, height: 8)
        }
        .onAppear {
            if color == FilTheme.online {
                withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: false)) {
                    isPulsing = true
                }
            }
        }
    }
}

// MARK: - Shimmer Modifier

struct ShimmerModifier: ViewModifier {
    @State private var phase: CGFloat = 0

    func body(content: Content) -> some View {
        content
            .overlay(
                LinearGradient(
                    gradient: Gradient(colors: [
                        Color.clear,
                        Color.white.opacity(0.05),
                        Color.clear,
                    ]),
                    startPoint: .leading,
                    endPoint: .trailing
                )
                .offset(x: phase)
                .onAppear {
                    withAnimation(.linear(duration: 1.5).repeatForever(autoreverses: false)) {
                        phase = 400
                    }
                }
            )
            .clipShape(RoundedRectangle(cornerRadius: 14))
    }
}

extension View {
    func shimmer() -> some View {
        modifier(ShimmerModifier())
    }
}
