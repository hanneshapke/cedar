import Foundation

/// Where cedar-chat is and how to talk to it. The access token lives in the Keychain; the rest
/// in `UserDefaults`.
struct ServerSettings: Equatable, Sendable {
    /// cedar-chat's address, as the browser reaches it.
    var serverURL: String
    /// Its `CHAT_ACCESS_TOKEN`; empty when the server runs without one.
    var accessToken: String
    /// The registry id of the model to use for new messages, or empty for the server's default.
    var model: String
    /// The reasoning level to ask for, or empty for the model's default.
    var reasoning: String
    /// Whether messages' runs plan (one of `planModes`), or empty for the server's Settings.
    var plan: String = ""

    static let defaultServerURL = "http://127.0.0.1:3000"

    /// The plan modes a message can ask for, with their labels: Cedar decides (`tool`, it proposes
    /// a plan with `propose_plan` when a request needs one), for longer requests, always, or never.
    /// The empty mode leaves it to the server's Settings.
    static let planModes: [PlanMode] = [
        PlanMode(id: "", label: "Server setting"),
        PlanMode(id: "tool", label: "When Cedar thinks it helps"),
        PlanMode(id: "auto", label: "For longer requests"),
        PlanMode(id: "on", label: "Always"),
        PlanMode(id: "off", label: "Never"),
    ]

    struct PlanMode: Identifiable, Sendable {
        let id: String
        let label: String
    }

    var isConfigured: Bool {
        !serverURL.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// Whether `other` points at another server, or at it with another token.
    func sameServer(as other: ServerSettings) -> Bool {
        serverURL.trimmingCharacters(in: .whitespacesAndNewlines) == other.serverURL.trimmingCharacters(in: .whitespacesAndNewlines)
            && accessToken.trimmingCharacters(in: .whitespacesAndNewlines) == other.accessToken.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private enum Key {
        static let serverURL = "chatServerURL"
        static let model = "model"
        static let reasoning = "reasoning"
        static let plan = "plan"
        static let accessToken = "accessToken"
    }

    static func load(defaults: UserDefaults = .standard) -> ServerSettings {
        ServerSettings(
            serverURL: defaults.string(forKey: Key.serverURL) ?? defaultServerURL,
            accessToken: Keychain.read(Key.accessToken) ?? "",
            model: defaults.string(forKey: Key.model) ?? "",
            reasoning: defaults.string(forKey: Key.reasoning) ?? "",
            plan: defaults.string(forKey: Key.plan) ?? ""
        )
    }

    func save(defaults: UserDefaults = .standard) {
        defaults.set(serverURL.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.serverURL)
        defaults.set(model.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.model)
        defaults.set(reasoning.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.reasoning)
        defaults.set(plan.trimmingCharacters(in: .whitespacesAndNewlines), forKey: Key.plan)
        Keychain.write(accessToken.trimmingCharacters(in: .whitespacesAndNewlines), for: Key.accessToken)
    }
}
