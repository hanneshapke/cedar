import Foundation
import Security

/// Minimal Keychain access for one generic-password item per account name.
enum Keychain {
    private static let service = "dev.cedar.ios"

    static func read(_ account: String) -> String? {
        var query = baseQuery(account)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: AnyObject?
        guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
              let data = result as? Data
        else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func write(_ value: String, for account: String) {
        guard !value.isEmpty else {
            SecItemDelete(baseQuery(account) as CFDictionary)
            return
        }
        let data = Data(value.utf8)
        let update = [kSecValueData as String: data] as CFDictionary
        if SecItemUpdate(baseQuery(account) as CFDictionary, update) == errSecItemNotFound {
            var query = baseQuery(account)
            query[kSecValueData as String] = data
            query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlock
            SecItemAdd(query as CFDictionary, nil)
        }
    }

    private static func baseQuery(_ account: String) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }
}
