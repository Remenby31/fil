import ComposableArchitecture
import SwiftUI
import UIKit

private enum SessionFilter: String, CaseIterable, Identifiable {
    case all
    case codex
    case claude
    case shell

    var id: Self { self }

    var title: LocalizedStringResource {
        switch self {
        case .all: "All"
        case .codex: "Codex"
        case .claude: "Claude"
        case .shell: "Shells"
        }
    }

    func matches(_ session: Session) -> Bool {
        switch self {
        case .all: true
        case .codex: session.processName == "Codex"
        case .claude: session.processName == "Claude Code"
        case .shell: session.isShellOnly
        }
    }
}

struct MachinesView: View {
    @Bindable var store: StoreOf<MachinesFeature>
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.horizontalSizeClass) private var horizontalSizeClass
    @State private var showSettings = false
    @State private var searchText = ""
    @State private var sessionFilter: SessionFilter = .all
    @State private var showOfflineMachines = false

    var body: some View {
        Group {
            if horizontalSizeClass == .regular {
                splitLayout
            } else {
                phoneLayout
            }
        }
        .onAppear { store.send(.onAppear) }
        .onDisappear { store.send(.onDisappear) }
        .sheet(isPresented: $showSettings) {
            SettingsView {
                showSettings = false
                store.send(.logoutTapped)
            }
        }
    }

    // MARK: - Layouts

    private var phoneLayout: some View {
        NavigationStack {
            rootContent(listStyle: .insetGrouped)
        }
        .fullScreenCover(item: $store.scope(state: \.terminal, action: \.terminal)) { terminalStore in
            TerminalSessionView(store: terminalStore)
        }
    }

    private var splitLayout: some View {
        NavigationSplitView {
            rootContent(listStyle: .sidebar)
                .navigationSplitViewColumnWidth(min: 300, ideal: 360, max: 440)
        } detail: {
            if let terminalStore = store.scope(state: \.terminal, action: \.terminal) {
                let presentedStore = terminalStore.scope(state: \.self, action: \.presented)
                TerminalSessionView(store: presentedStore, presentation: .embedded)
                    .id(presentedStore.session.id)
            } else {
                ContentUnavailableView(
                    "Select a Terminal",
                    systemImage: "terminal",
                    description: Text("Choose an active terminal from the sidebar.")
                )
            }
        }
        .navigationSplitViewStyle(.balanced)
    }

    private func rootContent(listStyle: SessionListStyle) -> some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()
            content(listStyle: listStyle)
        }
        .navigationTitle("Terminals")
        .navigationBarTitleDisplayMode(dynamicTypeSize.isAccessibilitySize ? .inline : .large)
        .searchable(text: $searchText, prompt: "Search projects, tools, or paths")
        .toolbar { mainToolbar }
        .safeAreaInset(edge: .top, spacing: 0) {
            if !store.isConnected {
                connectionBanner
            }
        }
        .safeAreaInset(edge: .bottom, spacing: 0) {
            if let error = store.errorMessage {
                errorBanner(error)
            }
        }
    }

    @ToolbarContentBuilder
    private var mainToolbar: some ToolbarContent {
        ToolbarItemGroup(placement: .topBarTrailing) {
            Menu {
                Picker("Terminal Type", selection: $sessionFilter) {
                    ForEach(SessionFilter.allCases) { filter in
                        Text(filter.title).tag(filter)
                    }
                }

                Toggle("Show Offline Machines", isOn: $showOfflineMachines)
            } label: {
                Image(systemName: sessionFilter == .all ? "line.3.horizontal.decrease" : "line.3.horizontal.decrease.circle.fill")
            }
            .accessibilityLabel("Filter terminals")

            Button {
                showSettings = true
            } label: {
                Image(systemName: "gearshape")
            }
            .accessibilityLabel("Settings")
        }
    }

    private enum SessionListStyle {
        case insetGrouped
        case sidebar
    }

    // MARK: - Content

    @ViewBuilder
    private func content(listStyle: SessionListStyle) -> some View {
        if store.machines.isEmpty && !store.isLoading {
            emptyState
        } else if store.isLoading && store.machines.isEmpty {
            loadingState
        } else if filteredMachines.isEmpty {
            ContentUnavailableView.search(text: searchText)
        } else {
            terminalList(style: listStyle)
        }
    }

    @ViewBuilder
    private func terminalList(style: SessionListStyle) -> some View {
        let list = List {
            listSections
        }
        .scrollContentBackground(.hidden)
        .background(FilTheme.void_)
        .refreshable { store.send(.refreshTapped) }

        switch style {
        case .insetGrouped:
            list.listStyle(.insetGrouped)
        case .sidebar:
            list.listStyle(.sidebar)
        }
    }

    @ViewBuilder
    private var listSections: some View {
        Section {
            Text(overviewText)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .listRowInsets(
                    EdgeInsets(top: 0, leading: 4, bottom: 8, trailing: 4)
                )
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
        }

        ForEach(filteredMachines) { machine in
            Section {
                if machine.activeSessions.isEmpty {
                    Text("No active terminals")
                        .font(.subheadline)
                        .foregroundStyle(.tertiary)
                        .frame(minHeight: 44, alignment: .leading)
                        .listRowBackground(FilTheme.surface.opacity(0.46))
                } else {
                    ForEach(machine.activeSessions) { session in
                        Button {
                            store.send(.sessionTapped(session))
                        } label: {
                            SessionRow(session: session)
                        }
                        .buttonStyle(SessionButtonStyle())
                        .contextMenu {
                            Button {
                                UIPasteboard.general.string = session.cwd
                            } label: {
                                Label("Copy Path", systemImage: "doc.on.doc")
                            }

                            Button {
                                store.send(.sessionTapped(session))
                            } label: {
                                Label("Open Terminal", systemImage: "terminal")
                            }
                        }
                        .listRowInsets(
                            EdgeInsets(top: 0, leading: 16, bottom: 0, trailing: 12)
                        )
                        .listRowBackground(FilTheme.surface.opacity(0.46))
                        .listRowSeparatorTint(FilTheme.cloud.opacity(0.08))
                    }
                }
            } header: {
                MachineSectionHeader(machine: machine)
            }
        }
    }

    private var filteredMachines: [Machine] {
        let query = searchText.trimmingCharacters(in: .whitespacesAndNewlines)
        return store.machines.compactMap { machine in
            let sessions = machine.activeSessions.filter { session in
                guard sessionFilter.matches(session) else { return false }
                guard !query.isEmpty else { return true }
                return [
                    session.projectName,
                    session.processName,
                    session.cwd,
                    machine.displayName,
                ].contains { $0.localizedCaseInsensitiveContains(query) }
            }

            let machineMatches = !query.isEmpty
                && machine.displayName.localizedCaseInsensitiveContains(query)
            let shouldShowOffline = showOfflineMachines || machine.status == .online
            guard shouldShowOffline, machineMatches || !sessions.isEmpty || query.isEmpty else {
                return nil
            }
            guard machine.status == .online || showOfflineMachines else { return nil }

            return Machine(
                id: machine.id,
                name: machine.name,
                status: machine.status,
                sessions: sessions
            )
        }
    }

    private var overviewText: String {
        let connectedMachines = store.machines.filter { $0.status == .online }
        let terminalCount = store.machines.flatMap(\.activeSessions).count
        let terminalSummary = terminalCount == 1
            ? String(localized: "1 active terminal")
            : String.localizedStringWithFormat(
                String(localized: "%lld active terminals"),
                terminalCount
            )

        if dynamicTypeSize.isAccessibilitySize {
            return terminalSummary
        }
        if terminalCount == 0 {
            return connectedMachines.isEmpty
                ? String(localized: "No connected machines")
                : String(localized: "No active terminals")
        }
        if connectedMachines.count == 1, let machine = connectedMachines.first {
            return String.localizedStringWithFormat(
                String(localized: "%@ on %@"),
                terminalSummary,
                machine.displayName
            )
        }
        return String.localizedStringWithFormat(
            String(localized: "%@ across %lld machines"),
            terminalSummary,
            connectedMachines.count
        )
    }

    // MARK: - States

    private var loadingState: some View {
        ProgressView("Loading terminals…")
            .tint(FilTheme.filGreen)
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .accessibilityLabel("Loading terminals")
    }

    private var emptyState: some View {
        ScrollView {
            VStack(spacing: 24) {
                ContentUnavailableView(
                    "No Machines",
                    systemImage: "desktopcomputer",
                    description: Text("Install fil on your Mac to see active terminals here.")
                )

                VStack(alignment: .leading, spacing: 12) {
                    setupStep("1", "brew install fil")
                    setupStep("2", "fil setup")
                    setupStep("3", "Restart your terminal")
                }
                .padding(16)
                .background(FilTheme.surface.opacity(0.46), in: RoundedRectangle(cornerRadius: 16))
                .padding(.horizontal, 24)
            }
            .padding(.top, 48)
        }
    }

    private func setupStep(_ number: String, _ code: String) -> some View {
        HStack(spacing: 12) {
            Text(number)
                .font(.caption2.bold().monospaced())
                .foregroundStyle(FilTheme.void_)
                .frame(width: 24, height: 24)
                .background(FilTheme.filGreen)
                .clipShape(Circle())

            Text(code)
                .font(.callout.monospaced())
                .foregroundStyle(.primary)
        }
        .accessibilityElement(children: .combine)
    }

    // MARK: - Banners

    private var connectionBanner: some View {
        HStack(spacing: 8) {
            Image(systemName: "wifi.slash")
                .font(.caption.weight(.medium))
                .accessibilityHidden(true)

            Text("Hub unreachable")
                .font(.subheadline.weight(.medium))

            Spacer()

            Button("Retry") {
                store.send(.refreshTapped)
            }
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(FilTheme.filGreen)
            .frame(minHeight: 44)
        }
        .foregroundStyle(FilTheme.warning)
        .padding(.horizontal, 16)
        .background(.ultraThinMaterial)
        .overlay(alignment: .bottom) {
            Divider().overlay(FilTheme.warning.opacity(0.24))
        }
    }

    private func errorBanner(_ message: String) -> some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(FilTheme.error)
                .font(.caption.weight(.medium))
                .accessibilityHidden(true)

            Text(message)
                .font(.footnote)
                .foregroundStyle(FilTheme.error)
                .lineLimit(2)

            Spacer()

            Button {
                store.send(.dismissError)
            } label: {
                Image(systemName: "xmark")
                    .font(.caption.weight(.semibold))
                    .frame(width: 44, height: 44)
            }
            .foregroundStyle(.secondary)
            .accessibilityLabel("Dismiss error")
        }
        .padding(.leading, 16)
        .background(.ultraThinMaterial)
        .overlay(alignment: .top) {
            Divider().overlay(FilTheme.error.opacity(0.2))
        }
    }
}

// MARK: - Machine Section

struct MachineSectionHeader: View {
    let machine: Machine
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        Group {
            if dynamicTypeSize.isAccessibilitySize {
                VStack(alignment: .leading, spacing: 6) {
                    HStack(spacing: 10) {
                        machineIcon
                        machineName
                    }
                    status
                        .padding(.leading, 28)
                }
            } else {
                HStack(spacing: 10) {
                    machineIcon
                    machineName

                    Spacer(minLength: 8)

                    status
                }
            }
        }
        .textCase(nil)
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(machine.displayName), \(String(localized: statusLabel))")
    }

    private var machineIcon: some View {
        Image(systemName: "desktopcomputer")
            .font(.system(size: 16, weight: .medium))
            .foregroundStyle(.secondary)
            .frame(width: 18)
            .accessibilityHidden(true)
    }

    private var machineName: some View {
        Text(machine.displayName)
            .font(.headline)
            .foregroundStyle(.primary)
            .lineLimit(dynamicTypeSize.isAccessibilitySize ? 3 : 1)
    }

    private var status: some View {
        HStack(spacing: 6) {
            Circle()
                .fill(statusColor)
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)

            Text(statusLabel)
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private var statusLabel: LocalizedStringResource {
        switch machine.status {
        case .online: "Connected"
        case .unreachable: "Unreachable"
        case .offline: "Offline"
        }
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
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize

    var body: some View {
        TimelineView(.periodic(from: .now, by: 60)) { context in
            Group {
                if dynamicTypeSize.isAccessibilitySize {
                    accessibilityRow(at: context.date)
                } else {
                    HStack(spacing: 12) {
                        terminalIcon

                        sessionLabels

                        Spacer(minLength: 10)

                        if let duration = session.duration(at: context.date) {
                            durationLabel(duration)
                        }

                        Image(systemName: "chevron.right")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(.tertiary)
                            .accessibilityHidden(true)
                    }
                }
            }
            .frame(maxWidth: .infinity, minHeight: 58, alignment: .leading)
            .contentShape(Rectangle())
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(session.accessibilityDescription(at: context.date))
            .accessibilityHint("Opens this terminal")
        }
    }

    private func accessibilityRow(at date: Date) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 10) {
                terminalIcon

                Text(session.projectName)
                    .font(.headline)
                    .foregroundStyle(.primary)
                    .lineLimit(3)
            }

            Text(session.listDetail)
                .font(.body)
                .foregroundStyle(.secondary)
                .lineLimit(3)

            if let duration = session.duration(at: date) {
                durationLabel(duration)
            }
        }
        .padding(.vertical, 8)
    }

    private var terminalIcon: some View {
        Image(systemName: "terminal")
            .font(.system(size: 16, weight: .medium))
            .symbolRenderingMode(.hierarchical)
            .foregroundStyle(.secondary)
            .frame(width: 24)
            .accessibilityHidden(true)
    }

    private var sessionLabels: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(session.projectName)
                .font(.body.weight(.semibold))
                .foregroundStyle(.primary)
                .lineLimit(dynamicTypeSize.isAccessibilitySize ? 2 : 1)

            if dynamicTypeSize.isAccessibilitySize {
                Text(session.listDetail)
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            } else {
                HStack(spacing: 4) {
                    Text(session.processName)
                        .fixedSize(horizontal: true, vertical: false)

                    Text("·")

                    Text(session.parentCompactPath)
                        .lineLimit(1)
                        .truncationMode(.head)
                }
                .font(.subheadline)
                .foregroundStyle(.secondary)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func durationLabel(_ duration: String) -> some View {
        Text(duration)
            .font(.caption)
            .monospacedDigit()
            .foregroundStyle(.secondary)
            .fixedSize(horizontal: true, vertical: false)
    }
}

// MARK: - Interaction

struct SessionButtonStyle: ButtonStyle {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(reduceMotion ? 1 : (configuration.isPressed ? 0.99 : 1))
            .opacity(configuration.isPressed ? 0.68 : 1)
            .animation(
                reduceMotion ? nil : .easeOut(duration: 0.1),
                value: configuration.isPressed
            )
    }
}
