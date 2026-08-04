import ActivityKit
import SwiftUI

struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss

    @State private var hubURL = TokenStorage.loadHubUrl()
    @State private var terminalFontSize = FilSharedStore.terminalFontSize
    @State private var liveActivitiesEnabled = FilActivityPreferences.current.isEnabled
    @State private var liveActivityPrivacy = FilActivityPreferences.current.privacy
    @State private var showLogoutConfirmation = false
    @State private var showDeleteConfirmation = false
    @State private var showSetupGuide = false
    @State private var isDeletingAccount = false
    @State private var accountError: String?
    @State private var hubStatusMessage: String?
    @State private var liveActivityPushEnabled: Bool?

    let onLogout: () -> Void

    var body: some View {
        NavigationStack {
            Form {
                connectionSection
                terminalSection
                liveActivitiesSection
                accountSection
                aboutSection
            }
            .scrollContentBackground(.hidden)
            .background(Color(uiColor: .systemGroupedBackground))
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        .presentationDetents([.large])
        .presentationDragIndicator(.visible)
        .confirmationDialog("Sign out of Fil?", isPresented: $showLogoutConfirmation) {
            Button("Sign Out", role: .destructive) {
                onLogout()
                dismiss()
            }
        }
        .confirmationDialog(
            "Delete your Fil account?",
            isPresented: $showDeleteConfirmation,
            titleVisibility: .visible
        ) {
            Button("Delete Account", role: .destructive) {
                deleteAccount()
            }
        } message: {
            Text("This permanently removes your account, registered machines, and Live Activity registrations.")
        }
        .alert("Couldn’t Delete Account", isPresented: accountErrorBinding) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(accountError ?? "Please try again.")
        }
        .fullScreenCover(isPresented: $showSetupGuide) {
            OnboardingView(isPresented: $showSetupGuide)
        }
        .task {
            liveActivityPushEnabled = try? await HubClient().health().liveActivityPushEnabled
        }
    }

    private var connectionSection: some View {
        Section {
            TextField("Hub URL", text: $hubURL)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .keyboardType(.URL)
                .font(.body.monospaced())

            Button("Save Hub URL") {
                saveHubURL()
            }

            if let hubStatusMessage {
                Text(hubStatusMessage)
                    .font(.footnote)
                    .foregroundStyle(hubStatusMessage == "Saved" ? FilTheme.filGreen : FilTheme.error)
            }
        } header: {
            Text("Connection")
        } footer: {
            Text("Changing the hub takes effect the next time Fil reconnects.")
        }
    }

    private var terminalSection: some View {
        Section("Terminal") {
            Stepper(
                value: $terminalFontSize,
                in: 10...24,
                step: 1
            ) {
                LabeledContent("Font Size", value: "\(Int(terminalFontSize)) pt")
            }
            .onChange(of: terminalFontSize) { _, size in
                FilSharedStore.terminalFontSize = size
            }
        }
    }

    private var liveActivitiesSection: some View {
        Section {
            Toggle("Allow Live Activities", systemImage: "livephoto", isOn: $liveActivitiesEnabled)
                .tint(FilTheme.filGreen)
                .onChange(of: liveActivitiesEnabled) { _, enabled in
                    saveLiveActivityPreferences()
                    if !enabled {
                        Task {
                            if #available(iOS 16.2, *) {
                                await FilActivityManager.shared.endAllImmediately()
                            }
                        }
                    }
                }

            if liveActivitiesEnabled {
                Picker("Lock Screen Privacy", selection: $liveActivityPrivacy) {
                    ForEach(FilActivityPrivacy.allCases, id: \.self) { privacy in
                        Text(privacy.title).tag(privacy)
                    }
                }
                .onChange(of: liveActivityPrivacy) { _, _ in
                    saveLiveActivityPreferences()
                }
            }
        } header: {
            Text("Live Activities")
        } footer: {
            if liveActivityPushEnabled == true {
                Text("Following is always started manually. Push updates continue when Fil is closed.")
            } else {
                Text("Following is always started manually. If Fil is closed, updates pause until the app reconnects.")
            }
        }
    }

    private var accountSection: some View {
        Section("Account") {
            LabeledContent {
                VStack(alignment: .trailing, spacing: 2) {
                    Text(providerName)
                    if let email = TokenStorage.loadEmail() {
                        Text(email)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            } label: {
                Label("Signed In With", systemImage: providerIcon)
            }

            Button("Sign Out", systemImage: "rectangle.portrait.and.arrow.right") {
                showLogoutConfirmation = true
            }

            Button("Delete Account", systemImage: "trash", role: .destructive) {
                showDeleteConfirmation = true
            }
            .disabled(isDeletingAccount)

            if isDeletingAccount {
                HStack {
                    ProgressView()
                    Text("Deleting account…")
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    private var aboutSection: some View {
        Section("About") {
            LabeledContent("Version", value: appVersion)

            Button("Show Setup Guide", systemImage: "questionmark.circle") {
                showSetupGuide = true
            }

            if let githubURL = URL(string: "https://github.com/Remenby31/fil") {
                Link(destination: githubURL) {
                    Label("GitHub", systemImage: "chevron.left.forwardslash.chevron.right")
                }
            }

            if let privacyURL = URL(string: "https://fil.sh/privacy") {
                Link(destination: privacyURL) {
                    Label("Privacy Policy", systemImage: "hand.raised")
                }
            }
        }
    }

    private var providerName: String {
        TokenStorage.loadProvider() == "apple" ? "Apple" : "GitHub"
    }

    private var providerIcon: String {
        TokenStorage.loadProvider() == "apple" ? "applelogo" : "network"
    }

    private var appVersion: String {
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? "—"
        let build = info?["CFBundleVersion"] as? String ?? "—"
        return "\(version) (\(build))"
    }

    private var accountErrorBinding: Binding<Bool> {
        Binding(
            get: { accountError != nil },
            set: { if !$0 { accountError = nil } }
        )
    }

    private func saveHubURL() {
        let trimmed = hubURL.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let components = URLComponents(string: trimmed),
              let scheme = components.scheme?.lowercased(),
              ["https", "http"].contains(scheme),
              components.host != nil else {
            hubStatusMessage = "Enter a valid HTTP or HTTPS URL"
            return
        }
        TokenStorage.saveHubUrl(trimmed.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
        hubStatusMessage = "Saved"
    }

    private func saveLiveActivityPreferences() {
        FilActivityPreferences(
            isEnabled: liveActivitiesEnabled,
            privacy: liveActivityPrivacy
        ).save()
    }

    private func deleteAccount() {
        guard !isDeletingAccount else { return }
        isDeletingAccount = true

        Task {
            do {
                guard let token = TokenStorage.loadToken() else {
                    throw HubError.httpError(401)
                }
                try await HubClient().deleteAccount(token: token)
                if #available(iOS 16.2, *) {
                    await FilActivityManager.shared.endAllImmediately()
                }
                FilSharedStore.clearWidgetSnapshot()
                isDeletingAccount = false
                onLogout()
                dismiss()
            } catch {
                isDeletingAccount = false
                accountError = error.localizedDescription
            }
        }
    }
}
