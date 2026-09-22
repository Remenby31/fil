import Foundation

struct FilActivityPreferences: Equatable, Sendable {
    static let enabledKey = "liveActivitiesEnabled"
    static let privacyKey = "liveActivityPrivacy"

    var isEnabled: Bool
    var privacy: FilActivityPrivacy

    static var current: Self {
        let defaults = UserDefaults.standard
        let enabled = defaults.object(forKey: enabledKey) as? Bool ?? true
        // An unconfigured shared policy defaults to Private, including upgrades.
        // A follow request must never write an older preference over a newer revocation.
        let privacy = FilSharedStore.activityPrivacy
        return Self(isEnabled: enabled, privacy: privacy)
    }

    func save() {
        FilSharedStore.saveActivityPrivacy(privacy)
        let defaults = UserDefaults.standard
        defaults.set(isEnabled, forKey: Self.enabledKey)
        defaults.set(privacy.rawValue, forKey: Self.privacyKey)
    }
}
