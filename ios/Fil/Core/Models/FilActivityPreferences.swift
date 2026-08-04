import Foundation

struct FilActivityPreferences: Equatable, Sendable {
    static let enabledKey = "liveActivitiesEnabled"
    static let privacyKey = "liveActivityPrivacy"

    var isEnabled: Bool
    var privacy: FilActivityPrivacy

    static var current: Self {
        let defaults = UserDefaults.standard
        let enabled = defaults.object(forKey: enabledKey) as? Bool ?? true
        let privacy = defaults.string(forKey: privacyKey)
            .flatMap(FilActivityPrivacy.init(rawValue:)) ?? .standard
        return Self(isEnabled: enabled, privacy: privacy)
    }

    func save() {
        let defaults = UserDefaults.standard
        defaults.set(isEnabled, forKey: Self.enabledKey)
        defaults.set(privacy.rawValue, forKey: Self.privacyKey)
    }
}
