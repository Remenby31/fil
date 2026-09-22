import ActivityKit
import SwiftUI
import UIKit

struct SettingsView: View {
    @Environment(\.dismiss) private var dismiss
    @Environment(\.scenePhase) private var scenePhase

    @State private var hubURL = TokenStorage.loadHubUrl()
    @State private var terminalFontSize = FilSharedStore.terminalFontSize
    @State private var liveActivitiesEnabled = FilActivityPreferences.current.isEnabled
    @State private var liveActivityPrivacy = FilActivityPreferences.current.privacy
    @State private var showLogoutConfirmation = false
    @State private var showDeleteConfirmation = false
    @State private var showSetupGuide = false
    @State private var isDeletingAccount = false
    @State private var accountError: String?
    @State private var hubWasSaved: Bool?
    @State private var liveActivityPushEnabled: Bool?
    @State private var systemAllowsLiveActivities = ActivityAuthorizationInfo().areActivitiesEnabled
    @State private var isApplyingPrivacy = false
    @State private var isSigningOut = false

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
            .disabled(isDeletingAccount || isSigningOut)
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
        .interactiveDismissDisabled(isApplyingPrivacy || isDeletingAccount || isSigningOut)
        .confirmationDialog("Sign out of Fil?", isPresented: $showLogoutConfirmation) {
            Button("Sign Out", role: .destructive) {
                isSigningOut = true
                Task {
                    await FilActivityManager.shared.endAllImmediately()
                    FilSharedStore.clearWidgetSnapshot()
                    onLogout()
                    dismiss()
                }
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
        .onChange(of: scenePhase) { _, phase in
            if phase == .active {
                systemAllowsLiveActivities = ActivityAuthorizationInfo().areActivitiesEnabled
            }
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

            if let hubWasSaved {
                Text(hubWasSaved ? LocalizedStringKey("Saved") : LocalizedStringKey("Enter a valid HTTP or HTTPS URL"))
                    .font(.footnote)
                    .foregroundStyle(hubWasSaved ? FilTheme.filGreenText : FilTheme.error)
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
                        removeActivitiesForPrivacyChange()
                    }
                }
                .disabled(isApplyingPrivacy)

            if liveActivitiesEnabled {
                Picker("Lock Screen Privacy", selection: $liveActivityPrivacy) {
                    ForEach(FilActivityPrivacy.allCases, id: \.self) { privacy in
                        Text(privacy.title).tag(privacy)
                    }
                }
                .onChange(of: liveActivityPrivacy) { _, _ in
                    // The shared policy/revision changes synchronously, before any await.
                    saveLiveActivityPreferences()
                    removeActivitiesForPrivacyChange()
                }
                .disabled(isApplyingPrivacy)
            }

            if isApplyingPrivacy {
                ProgressView("Removing current Live Activities…")
            }
            if !systemAllowsLiveActivities {
                Label("Live Activities are disabled in iOS Settings", systemImage: "info.circle")
                    .font(.footnote)
                if let settingsURL = URL(string: UIApplication.openSettingsURLString) {
                    Link("Open iOS Settings", destination: settingsURL)
                }
            }
        } header: {
            Text("Live Activities")
        } footer: {
            VStack(alignment: .leading, spacing: 6) {
                Text("Privacy also applies to widgets. Private hides machine, project and process names. Changing privacy removes current Live Activities; follow a terminal again to apply it.")
                if liveActivityPushEnabled == true {
                    Text("This hub supports push updates. Delivery can be delayed; check the last update time. Following is always started manually.")
                } else if liveActivityPushEnabled == false {
                    Text("This hub does not provide push updates. Information may become out of date while Fil is closed.")
                } else {
                    Text("Push availability could not be confirmed. Information may become out of date while Fil is closed.")
                }
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

            if let privacyURL = URL(string: "https://fil.remenby.fr/privacy") {
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
            hubWasSaved = false
            return
        }
        TokenStorage.saveHubUrl(trimmed.trimmingCharacters(in: CharacterSet(charactersIn: "/")))
        hubWasSaved = true
    }

    private func saveLiveActivityPreferences() {
        FilActivityPreferences(
            isEnabled: liveActivitiesEnabled,
            privacy: liveActivityPrivacy
        ).save()
    }

    private func removeActivitiesForPrivacyChange() {
        isApplyingPrivacy = true
        // Privacy revocation must finish even if the Settings view goes away.
        Task {
            await FilActivityManager.shared.endAllImmediately()
            isApplyingPrivacy = false
        }
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
