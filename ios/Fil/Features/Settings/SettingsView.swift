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
        NavigationStack {
            ZStack {
                FilTheme.void_.ignoresSafeArea()

                List {
                    hubSection
                    terminalSection
                    notificationsSection
                    accountSection
                    aboutSection
                }
                .scrollContentBackground(.hidden)
                .listStyle(.insetGrouped)
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbarColorScheme(.dark, for: .navigationBar)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Done") { dismiss() }
                        .foregroundStyle(FilTheme.filGreen)
                }
            }
        }
        .preferredColorScheme(.dark)
    }

    // MARK: - Sections

    private var hubSection: some View {
        Section {
            HStack {
                Label("Hub URL", systemImage: "server.rack")
                    .foregroundStyle(FilTheme.cloud)
                Spacer()
                TextField("https://hub.fil.sh", text: $hubUrl)
                    .multilineTextAlignment(.trailing)
                    .foregroundStyle(FilTheme.cloud.opacity(0.6))
                    .font(.system(size: 14, design: .monospaced))
                    .onSubmit {
                        TokenStorage.saveHubUrl(hubUrl)
                    }
            }
            .listRowBackground(FilTheme.surface)
        } header: {
            Text("Connection")
        }
    }

    private var terminalSection: some View {
        Section {
            HStack {
                Label("Font size", systemImage: "textformat.size")
                    .foregroundStyle(FilTheme.cloud)
                Spacer()
                Text("\(Int(fontSize))pt")
                    .foregroundStyle(FilTheme.cloud.opacity(0.5))
                    .font(.system(size: 14, design: .monospaced))
                Stepper("", value: $fontSize, in: 10...24, step: 1)
                    .labelsHidden()
            }
            .listRowBackground(FilTheme.surface)
        } header: {
            Text("Terminal")
        }
    }

    private var notificationsSection: some View {
        Section {
            Toggle(isOn: $notificationsEnabled) {
                Label("Notifications", systemImage: "bell.fill")
                    .foregroundStyle(FilTheme.cloud)
            }
            .tint(FilTheme.filGreen)
            .listRowBackground(FilTheme.surface)

            if notificationsEnabled {
                Toggle(isOn: $notifyCommandFinished) {
                    Text("Command finished")
                        .foregroundStyle(FilTheme.cloud)
                }
                .tint(FilTheme.filGreen)
                .listRowBackground(FilTheme.surface)

                Toggle(isOn: $notifyPromptWaiting) {
                    Text("Prompt waiting")
                        .foregroundStyle(FilTheme.cloud)
                }
                .tint(FilTheme.filGreen)
                .listRowBackground(FilTheme.surface)

                Toggle(isOn: $notifyErrors) {
                    Text("Errors")
                        .foregroundStyle(FilTheme.cloud)
                }
                .tint(FilTheme.filGreen)
                .listRowBackground(FilTheme.surface)
            }
        } header: {
            Text("Notifications")
        }
    }

    private var accountSection: some View {
        Section {
            Button {
                showLogoutConfirm = true
            } label: {
                Label("Sign out", systemImage: "rectangle.portrait.and.arrow.right")
                    .foregroundStyle(FilTheme.warning)
            }
            .listRowBackground(FilTheme.surface)
            .confirmationDialog("Sign out?", isPresented: $showLogoutConfirm) {
                Button("Sign out", role: .destructive) {
                    onLogout()
                }
            }

            Button {
                showDeleteConfirm = true
            } label: {
                Label("Delete account", systemImage: "trash")
                    .foregroundStyle(FilTheme.error)
            }
            .listRowBackground(FilTheme.surface)
            .confirmationDialog("Delete your account? This cannot be undone.", isPresented: $showDeleteConfirm) {
                Button("Delete account", role: .destructive) {
                    // TODO: call hub API to delete account
                    onLogout()
                }
            }
        } header: {
            Text("Account")
        }
    }

    private var aboutSection: some View {
        Section {
            HStack {
                Label("Version", systemImage: "info.circle")
                    .foregroundStyle(FilTheme.cloud)
                Spacer()
                Text("0.1.0")
                    .foregroundStyle(FilTheme.cloud.opacity(0.4))
            }
            .listRowBackground(FilTheme.surface)

            Link(destination: URL(string: "https://github.com/Remenby31/fil")!) {
                Label("GitHub", systemImage: "chevron.left.forwardslash.chevron.right")
                    .foregroundStyle(FilTheme.cloud)
            }
            .listRowBackground(FilTheme.surface)

            Link(destination: URL(string: "https://fil.sh/privacy")!) {
                Label("Privacy Policy", systemImage: "hand.raised")
                    .foregroundStyle(FilTheme.cloud)
            }
            .listRowBackground(FilTheme.surface)
        } header: {
            Text("About")
        }
    }
}
