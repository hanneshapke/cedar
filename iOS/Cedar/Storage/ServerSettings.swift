import Foundation

/// Where cedar is and how to talk to it. The API key lives in the Keychain; the rest in
/// `UserDefaults`.
struct ServerSettings: Equatable, Sendable {
    var serverURL: String
    var apiKey: String
    /// Sent as `model` on new conversations when not empty; otherwise cedar's `LLM_MODEL` is used.
    var model: String
    /// Sent as `system` on new conversations when not empty; otherwise cedar's default prompt.
    var systemPrompt: String

    var isConfigured: Bool {
        !serverURL.trimmingCharacters(in: .whitespaces).isEmpty
            && !apiKey.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private enum Key {
        static let serverURL = "serverURL"
        static let model = "model"
        static let systemPrompt = "systemPrompt"
        static let apiKey = "apiKey"
    }

    static func load(defaults: UserDefaults = .standard) -> ServerSettings {
        ServerSettings(
            serverURL: defaults.string(forKey: Key.serverURL) ?? "http://127.0.0.1:8080",
            apiKey: Keychain.read(Key.apiKey) ?? "",
            model: defaults.string(forKey: Key.model) ?? "",
            systemPrompt: defaults.string(forKey: Key.systemPrompt) ?? ""
        )
    }

    func save(defaults: UserDefaults = .standard) {
        defaults.set(serverURL.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.serverURL)
        defaults.set(model.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.model)
        defaults.set(systemPrompt, forKey: Key.systemPrompt)
        Keychain.write(apiKey.trimmingCharacters(in: .whitespacesAndNewlines), for: Key.apiKey)
    }
}
