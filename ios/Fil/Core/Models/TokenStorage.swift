import Foundation
import Security

enum TokenStorage {
    private static let service = "sh.fil.app"
    private static let tokenKey = "auth_token"
    private static let hubUrlKey = "hub_url"

    static func saveToken(_ token: String) {
        save(key: tokenKey, value: token)
    }

    static func loadToken() -> String? {
        load(key: tokenKey)
    }

    static func clearToken() {
        delete(key: tokenKey)
    }

    static func saveHubUrl(_ url: String) {
        save(key: hubUrlKey, value: url)
    }

    static func loadHubUrl() -> String {
        load(key: hubUrlKey) ?? "https://hub.fil.sh"
    }

    // MARK: - Keychain Helpers

    private static func save(key: String, value: String) {
        let data = Data(value.utf8)
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
        var addQuery = query
        addQuery[kSecValueData as String] = data
        SecItemAdd(addQuery as CFDictionary, nil)
    }

    private static func load(key: String) -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: AnyObject?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        guard status == errSecSuccess, let data = result as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    private static func delete(key: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: key,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
