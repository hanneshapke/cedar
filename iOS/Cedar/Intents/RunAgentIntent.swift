import AppIntents
import Foundation

// "Run Cedar Agent" in the Shortcuts app: this device asks the server to start one of the
// agents (`POST /api/agents/{id}/trigger`), by hand, from Siri or from one of the Shortcuts app's
// own automations (arriving at or leaving a place, CarPlay connecting, a time of day). The app
// itself watches nothing: no location, no extension. It runs without the app's window, so it
// reads the server's address and the Keychain's token itself, as `AppModel` does at launch.

/// An agent, as the action's Agent parameter lists it.
struct AgentEntity: AppEntity {
    static var typeDisplayRepresentation: TypeDisplayRepresentation = TypeDisplayRepresentation(name: "Agent")
    static var defaultQuery = AgentEntityQuery()

    let id: String
    let title: String
    /// What starts it otherwise, or that it is paused.
    let detail: String

    init(id: String, title: String, detail: String) {
        self.id = id
        self.title = title
        self.detail = detail
    }

    init(_ agent: Agent) {
        self.init(id: agent.id, title: agent.title,
                  detail: agent.enabled ? agent.trigger.summary : L10n.string("Paused"))
    }

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(title)", subtitle: "\(detail)")
    }
}

/// Lists the server's agents for the parameter, and finds the picked ones again when a shortcut
/// runs. Any agent can be asked for a run, not only one that waits for the device; a paused one
/// is listed too, and says so when it is run.
struct AgentEntityQuery: EntityQuery {
    func entities(for identifiers: [AgentEntity.ID]) async throws -> [AgentEntity] {
        let wanted = Set(identifiers)
        return try await suggestedEntities().filter { wanted.contains($0.id) }
    }

    func suggestedEntities() async throws -> [AgentEntity] {
        do {
            return try await RunAgentIntent.client().agents().agents.map { AgentEntity($0) }
        } catch {
            throw RunAgentError(message: error.localizedDescription)
        }
    }
}

/// Why the action couldn't ask, in words the Shortcuts app shows.
struct RunAgentError: Error, CustomLocalizedStringResourceConvertible {
    let message: String

    var localizedStringResource: LocalizedStringResource { "\(message)" }
}

/// Asks the server to start an agent now, with an optional text handed to the run as data. Says
/// that it started, or why it is held back (still running, too soon after the last run, the
/// day's runs used up): a run that is held back is not queued.
struct RunAgentIntent: AppIntent {
    static var title: LocalizedStringResource = "Run Cedar Agent"
    static var description: IntentDescription? = IntentDescription("Starts one of your Cedar agents now. Text you pass in goes to the run as data.")

    @Parameter(title: "Agent")
    var agent: AgentEntity

    @Parameter(title: "Text", description: "Handed to the run as data: a note, a link, what a shortcut found.")
    var text: String?

    static var parameterSummary: some ParameterSummary {
        Summary("Run \(\.$agent)") {
            \.$text
        }
    }

    /// The client for the server in Settings, with the token from the Keychain.
    static func client() throws -> ChatClient {
        try ChatClient(settings: ServerSettings.load())
    }

    func perform() async throws -> some IntentResult & ProvidesDialog {
        let passed = text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let message: String
        do {
            let result = try await Self.client().triggerAgent(id: agent.id, source: "shortcut",
                                                              text: passed.isEmpty ? nil : passed)
            message = result.message(agent: agent.title)
        } catch let error as ChatError where error.status == 409 {
            throw RunAgentError(message: L10n.format("%@ is paused. Resume it in Cedar to run it.", agent.title))
        } catch let error as ChatError where error.status == 404 {
            throw RunAgentError(message: L10n.format("%@ no longer exists on the server.", agent.title))
        } catch {
            throw RunAgentError(message: error.localizedDescription)
        }
        return .result(dialog: "\(message)")
    }
}

/// Puts the action in the Shortcuts app and Spotlight without the user building a shortcut first.
struct CedarShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(intent: RunAgentIntent(),
                    phrases: ["Run an agent in \(.applicationName)"],
                    shortTitle: "Run Agent",
                    systemImageName: "play.circle")
    }
}
