import SwiftUI

struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @State private var hubUrl: String = TokenStorage.loadHubUrl()
    @State private var fontSize: Double = 14
    @State private var notificationsEnabled = true
    @State private var notifyCommandFinished = true
    @State private var notifyPromptWaiting = true
    @State private var notifyErrors = true
    @State private var showLogoutConfirm = false
    @State private var showDeleteConfirm = false

    let onLogout: () -> Void

    var body: some View {
        ZStack {
            FilTheme.void_.ignoresSafeArea()

            VStack(spacing: 0) {
                // Compact header
                HStack {
                    Text("Settings")
                        .font(.system(size: 18, weight: .semibold))
                        .foregroundStyle(FilTheme.cloud)

                    Spacer()

                    Button { dismiss() } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 13, weight: .semibold))
                            .foregroundStyle(FilTheme.cloud.opacity(0.4))
                            .frame(width: 30, height: 30)
                            .background(FilTheme.surface)
                            .clipShape(Circle())
                    }
                }
                .padding(.horizontal, 20)
                .padding(.top, 20)
                .padding(.bottom, 16)

                ScrollView {
                    VStack(spacing: 24) {
                        connectionSection
                        terminalSection
                        notificationsSection
                        accountSection
                        aboutSection
                    }
                    .padding(.horizontal, 16)
                    .padding(.bottom, 40)
                }
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .preferredColorScheme(.dark)
    }

    // MARK: - Sections

    private var connectionSection: some View {
        SettingsSection(title: "Connection") {
            SettingsRow {
                HStack {
                    Label("Hub", systemImage: "server.rack")
                        .font(.system(size: 14))
                        .foregroundStyle(FilTheme.cloud)
                    Spacer()
                    TextField("https://fil.remenby.fr", text: $hubUrl)
                        .multilineTextAlignment(.trailing)
                        .foregroundStyle(FilTheme.filGreen)
                        .font(.system(size: 13, design: .monospaced))
                        .onSubmit { TokenStorage.saveHubUrl(hubUrl) }
                }
            }
        }
    }

    private var terminalSection: some View {
        SettingsSection(title: "Terminal") {
            SettingsRow {
                HStack {
                    Label("Font size", systemImage: "textformat.size")
                        .font(.system(size: 14))
                        .foregroundStyle(FilTheme.cloud)
                    Spacer()
                    Text("\(Int(fontSize))pt")
                        .foregroundStyle(FilTheme.cloud.opacity(0.4))
                        .font(.system(size: 13, design: .monospaced))
                    Stepper("", value: $fontSize, in: 10...24, step: 1)
                        .labelsHidden()
                        .tint(FilTheme.filGreen)
                }
            }
        }
    }

    private var notificationsSection: some View {
        SettingsSection(title: "Notifications") {
            VStack(spacing: 0) {
                SettingsToggle("Notifications", icon: "bell.fill", isOn: $notificationsEnabled)

                if notificationsEnabled {
                    Divider().overlay(FilTheme.cloud.opacity(0.05))
                    SettingsToggle("Command finished", icon: "checkmark.circle", isOn: $notifyCommandFinished)
                    Divider().overlay(FilTheme.cloud.opacity(0.05))
                    SettingsToggle("Prompt waiting", icon: "questionmark.circle", isOn: $notifyPromptWaiting)
                    Divider().overlay(FilTheme.cloud.opacity(0.05))
                    SettingsToggle("Errors", icon: "exclamationmark.triangle", isOn: $notifyErrors)
                }
            }
            .padding(14)
            .background(FilTheme.surface.opacity(0.6))
            .clipShape(RoundedRectangle(cornerRadius: 14))
        }
    }

    private var accountSection: some View {
        let provider = TokenStorage.loadProvider()
        let email = TokenStorage.loadEmail()

        return SettingsSection(title: "Account") {
            VStack(spacing: 8) {
                // Connected account info
                VStack(spacing: 0) {
                    HStack(spacing: 10) {
                        Image(systemName: provider == "apple" ? "applelogo" : "network")
                            .font(.system(size: 16))
                            .foregroundStyle(FilTheme.filGreen)
                            .frame(width: 32, height: 32)
                            .background(FilTheme.filGreen.opacity(0.1))
                            .clipShape(Circle())

                        VStack(alignment: .leading, spacing: 2) {
                            Text("Signed in with \(provider == "apple" ? "Apple" : "GitHub")")
                                .font(.system(size: 14, weight: .medium))
                                .foregroundStyle(FilTheme.cloud)

                            if let email {
                                Text(email)
                                    .font(.system(size: 12))
                                    .foregroundStyle(FilTheme.cloud.opacity(0.4))
                            }
                        }

                        Spacer()
                    }
                }
                .padding(14)
                .background(FilTheme.surface.opacity(0.6))
                .clipShape(RoundedRectangle(cornerRadius: 14))

                Button {
                    showLogoutConfirm = true
                } label: {
                    HStack {
                        Label("Sign out", systemImage: "rectangle.portrait.and.arrow.right")
                            .font(.system(size: 14))
                            .foregroundStyle(FilTheme.warning)
                        Spacer()
                    }
                    .padding(14)
                    .background(FilTheme.surface.opacity(0.6))
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                }
                .confirmationDialog("Sign out?", isPresented: $showLogoutConfirm) {
                    Button("Sign out", role: .destructive) { onLogout() }
                }

                Button {
                    showDeleteConfirm = true
                } label: {
                    HStack {
                        Label("Delete account", systemImage: "trash")
                            .font(.system(size: 14))
                            .foregroundStyle(FilTheme.error.opacity(0.8))
                        Spacer()
                    }
                    .padding(14)
                    .background(FilTheme.surface.opacity(0.6))
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                }
                .confirmationDialog("Delete your account? This cannot be undone.", isPresented: $showDeleteConfirm) {
                    Button("Delete account", role: .destructive) { onLogout() }
                }
            }
        }
    }

    private var aboutSection: some View {
        SettingsSection(title: "About") {
            VStack(spacing: 0) {
                SettingsRow {
                    HStack {
                        Label("Version", systemImage: "info.circle")
                            .font(.system(size: 14))
                            .foregroundStyle(FilTheme.cloud)
                        Spacer()
                        Text("0.1.0")
                            .font(.system(size: 13, design: .monospaced))
                            .foregroundStyle(FilTheme.cloud.opacity(0.3))
                    }
                }

                Divider().overlay(FilTheme.cloud.opacity(0.05))

                Link(destination: URL(string: "https://github.com/Remenby31/fil")!) {
                    SettingsLinkRow(title: "GitHub", icon: "chevron.left.forwardslash.chevron.right")
                }

                Divider().overlay(FilTheme.cloud.opacity(0.05))

                Link(destination: URL(string: "https://fil.sh/privacy")!) {
                    SettingsLinkRow(title: "Privacy Policy", icon: "hand.raised")
                }
            }
            .background(FilTheme.surface.opacity(0.6))
            .clipShape(RoundedRectangle(cornerRadius: 14))
        }
    }
}

// MARK: - Settings Components

struct SettingsSection<Content: View>: View {
    let title: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title.uppercased())
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(FilTheme.cloud.opacity(0.3))
                .tracking(1)
                .padding(.leading, 4)

            content
        }
    }
}

struct SettingsRow<Content: View>: View {
    @ViewBuilder let content: Content

    var body: some View {
        content
            .padding(14)
            .background(FilTheme.surface.opacity(0.6))
            .clipShape(RoundedRectangle(cornerRadius: 14))
    }
}

struct SettingsToggle: View {
    let title: String
    let icon: String
    @Binding var isOn: Bool

    init(_ title: String, icon: String, isOn: Binding<Bool>) {
        self.title = title
        self.icon = icon
        self._isOn = isOn
    }

    var body: some View {
        Toggle(isOn: $isOn) {
            Label(title, systemImage: icon)
                .font(.system(size: 14))
                .foregroundStyle(FilTheme.cloud)
        }
        .tint(FilTheme.filGreen)
        .padding(.horizontal, 2)
        .padding(.vertical, 4)
    }
}

struct SettingsLinkRow: View {
    let title: String
    let icon: String

    var body: some View {
        HStack {
            Label(title, systemImage: icon)
                .font(.system(size: 14))
                .foregroundStyle(FilTheme.cloud)
            Spacer()
            Image(systemName: "arrow.up.right")
                .font(.system(size: 10))
                .foregroundStyle(FilTheme.cloud.opacity(0.2))
        }
        .padding(14)
    }
}
