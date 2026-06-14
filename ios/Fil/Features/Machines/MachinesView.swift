import ComposableArchitecture
import SwiftUI

struct MachinesView: View {
    @Bindable var store: StoreOf<MachinesFeature>
    @State private var showSettings = false

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            VStack(spacing: 0) {
                if !store.isConnected {
                    connectionBanner
                }

                if store.machines.isEmpty && !store.isLoading {
                    emptyState
                } else if store.isLoading && store.machines.isEmpty {
                    loadingSkeleton
                } else {
                    mainContent
                }
            }
        }
        .onAppear { store.send(.onAppear) }
        .fullScreenCover(item: $store.scope(state: \.terminal, action: \.terminal)) { terminalStore in
            TerminalSessionView(store: terminalStore)
        }
        .sheet(isPresented: $showSettings) {
            SettingsView {
                showSettings = false
                store.send(.logoutTapped)
            }
        }
        .preferredColorScheme(.dark)
    }

    // MARK: - Main Content

    private var mainContent: some View {
        VStack(spacing: 0) {
            // Compact top bar
            topBar

            ScrollView {
                VStack(spacing: 16) {
                    // Status summary
                    statusRow

                    // Machine cards
                    ForEach(store.machines) { machine in
                        MachineCard(machine: machine) { session in
                            store.send(.sessionTapped(session))
                        }
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 4)
                .padding(.bottom, 32)
            }
            .refreshable { store.send(.refreshTapped) }

            if let error = store.errorMessage {
                errorBanner(error)
            }
        }
    }

    // MARK: - Top Bar

    private var topBar: some View {
        HStack(alignment: .center) {
            Text("fil")
                .font(.system(size: 22, weight: .light))
                .foregroundStyle(FilTheme.cloud)
            + Text(".sh")
                .font(.system(size: 22, weight: .light))
                .foregroundStyle(FilTheme.filGreen)

            Spacer()

            Button {
                showSettings = true
            } label: {
                Image(systemName: "gearshape")
                    .font(.system(size: 16))
                    .foregroundStyle(FilTheme.cloud.opacity(0.4))
                    .frame(width: 36, height: 36)
                    .background(FilTheme.surface)
                    .clipShape(Circle())
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 8)
        .padding(.bottom, 12)
    }

    // MARK: - Status Row

    private var statusRow: some View {
        let onlineCount = store.machines.filter { $0.status == .online }.count
        let sessionCount = store.machines.flatMap { $0.sessions }.count

        return HStack(spacing: 16) {
            StatusPill(
                icon: "desktopcomputer",
                value: "\(onlineCount)",
                label: onlineCount == 1 ? "machine" : "machines",
                color: onlineCount > 0 ? FilTheme.filGreen : FilTheme.offline
            )

            StatusPill(
                icon: "terminal",
                value: "\(sessionCount)",
                label: sessionCount == 1 ? "session" : "sessions",
                color: sessionCount > 0 ? FilTheme.filGreen : FilTheme.offline
            )

            Spacer()
        }
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

    // MARK: - Loading Skeleton

    private var loadingSkeleton: some View {
        VStack(spacing: 0) {
            topBar
            VStack(spacing: 12) {
                ForEach(0..<3, id: \.self) { _ in
                    RoundedRectangle(cornerRadius: 16)
                        .fill(FilTheme.surface)
                        .frame(height: 110)
                        .shimmer()
                }
            }
            .padding(.horizontal, 16)
            .padding(.top, 20)
            Spacer()
        }
    }

    // MARK: - Empty State

    private var emptyState: some View {
        VStack(spacing: 0) {
            topBar
            Spacer()

            VStack(spacing: 24) {
                ZStack {
                    Circle()
                        .fill(FilTheme.filGreen.opacity(0.08))
                        .frame(width: 96, height: 96)

                    Image(systemName: "desktopcomputer")
                        .font(.system(size: 36))
                        .foregroundStyle(FilTheme.filGreen.opacity(0.5))
                }

                VStack(spacing: 8) {
                    Text("No machines yet")
                        .font(.system(size: 20, weight: .semibold))
                        .foregroundStyle(FilTheme.cloud)

                    Text("Install fil on your Mac to see\nyour sessions here.")
                        .font(.system(size: 15))
                        .foregroundStyle(FilTheme.cloud.opacity(0.4))
                        .multilineTextAlignment(.center)
                }

                VStack(alignment: .leading, spacing: 10) {
                    setupStep("1", "brew install fil")
                    setupStep("2", "fil setup")
                    setupStep("3", "Restart your terminal")
                }
                .padding(20)
                .background(FilTheme.depth)
                .clipShape(RoundedRectangle(cornerRadius: 14))
            }
            .padding(.horizontal, 40)

            Spacer()
            Spacer()
        }
    }

    private func setupStep(_ num: String, _ code: String) -> some View {
        HStack(spacing: 12) {
            Text(num)
                .font(.system(size: 11, weight: .bold, design: .monospaced))
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
                .font(.system(size: 12))

            Text(message)
                .font(.system(size: 13))
                .foregroundStyle(FilTheme.error.opacity(0.8))
                .lineLimit(1)

            Spacer()

            Button { store.send(.dismissError) } label: {
                Image(systemName: "xmark")
                    .font(.system(size: 10))
                    .foregroundStyle(FilTheme.cloud.opacity(0.3))
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(FilTheme.error.opacity(0.06))
    }
}

// MARK: - Status Pill

struct StatusPill: View {
    let icon: String
    let value: String
    let label: String
    let color: Color

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: icon)
                .font(.system(size: 12))
                .foregroundStyle(color)

            Text(value)
                .font(.system(size: 15, weight: .semibold, design: .monospaced))
                .foregroundStyle(FilTheme.cloud)

            Text(label)
                .font(.system(size: 13))
                .foregroundStyle(FilTheme.cloud.opacity(0.35))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(FilTheme.surface)
        .clipShape(Capsule())
    }
}

// MARK: - Machine Card

struct MachineCard: View {
    let machine: Machine
    let onSessionTap: (Session) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            // Machine header
            HStack(spacing: 10) {
                ZStack {
                    RoundedRectangle(cornerRadius: 8)
                        .fill(statusColor.opacity(0.1))
                        .frame(width: 34, height: 34)

                    Image(systemName: machine.status == .offline ? "desktopcomputer" : "desktopcomputer")
                        .font(.system(size: 15))
                        .foregroundStyle(statusColor)
                }

                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 6) {
                        Text(machine.name)
                            .font(.system(size: 15, weight: .semibold))
                            .foregroundStyle(FilTheme.cloud)

                        PulsingDot(color: statusColor)
                    }

                    Text(machine.status == .offline ? "Offline" : "\(machine.sessions.count) active")
                        .font(.system(size: 12))
                        .foregroundStyle(FilTheme.cloud.opacity(0.35))
                }

                Spacer()
            }

            // Sessions
            if !machine.sessions.isEmpty {
                VStack(spacing: 6) {
                    ForEach(machine.sessions) { session in
                        Button { onSessionTap(session) } label: {
                            SessionRow(session: session)
                        }
                        .buttonStyle(SessionButtonStyle())
                    }
                }
            }
        }
        .padding(16)
        .background(FilTheme.surface.opacity(0.6))
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .overlay(
            RoundedRectangle(cornerRadius: 16)
                .stroke(statusColor.opacity(machine.status == .online ? 0.08 : 0.03), lineWidth: 1)
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
        HStack(spacing: 10) {
            RoundedRectangle(cornerRadius: 3)
                .fill(FilTheme.filGreen.opacity(0.4))
                .frame(width: 3, height: 28)

            VStack(alignment: .leading, spacing: 2) {
                Text(session.shell)
                    .font(.system(size: 14, weight: .medium, design: .monospaced))
                    .foregroundStyle(FilTheme.cloud)

                Text(session.cwd)
                    .font(.system(size: 12))
                    .foregroundStyle(FilTheme.cloud.opacity(0.3))
                    .lineLimit(1)
            }

            Spacer()

            Text(session.duration)
                .font(.system(size: 11, design: .monospaced))
                .foregroundStyle(FilTheme.cloud.opacity(0.25))

            Image(systemName: "chevron.right")
                .font(.system(size: 10, weight: .semibold))
                .foregroundStyle(FilTheme.filGreen.opacity(0.3))
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(FilTheme.elevated.opacity(0.5))
        .clipShape(RoundedRectangle(cornerRadius: 10))
    }
}

// MARK: - Session Button Style

struct SessionButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.97 : 1.0)
            .opacity(configuration.isPressed ? 0.8 : 1.0)
            .animation(.easeInOut(duration: 0.15), value: configuration.isPressed)
    }
}

// MARK: - Pulsing Dot

struct PulsingDot: View {
    let color: Color
    @State private var isPulsing = false

    var body: some View {
        ZStack {
            if color == FilTheme.online {
                Circle()
                    .fill(color.opacity(0.3))
                    .frame(width: 12, height: 12)
                    .scaleEffect(isPulsing ? 1.5 : 1.0)
                    .opacity(isPulsing ? 0 : 0.4)
            }

            Circle()
                .fill(color)
                .frame(width: 7, height: 7)
        }
        .onAppear {
            if color == FilTheme.online {
                withAnimation(.easeInOut(duration: 1.8).repeatForever(autoreverses: false)) {
                    isPulsing = true
                }
            }
        }
    }
}

// MARK: - Shimmer

struct ShimmerModifier: ViewModifier {
    @State private var phase: CGFloat = -200

    func body(content: Content) -> some View {
        content
            .overlay(
                LinearGradient(
                    colors: [.clear, Color.white.opacity(0.04), .clear],
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
            .clipShape(RoundedRectangle(cornerRadius: 16))
    }
}

extension View {
    func shimmer() -> some View {
        modifier(ShimmerModifier())
    }
}
