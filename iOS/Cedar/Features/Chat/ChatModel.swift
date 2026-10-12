import Foundation
import Observation

/// One conversation's exchanges, kept up to date from cedar's event streams.
@MainActor
@Observable
final class ChatModel {
    /// Stable identity, also used as the selection key while the chat is still a draft.
    let key = UUID().uuidString
    private(set) var conversationId: String?
    private(set) var exchanges: [Exchange] = []
    /// The message being sent, shown until cedar accepts the run.
    private(set) var pendingPrompt: String?
    private(set) var isLoading = false
    var errorMessage: String?
    var draft = ""

    @ObservationIgnored private weak var app: AppModel?
    @ObservationIgnored private var followers: [String: Task<Void, Never>] = [:]

    init(app: AppModel, conversationId: String? = nil) {
        self.app = app
        self.conversationId = conversationId
    }

    var isActive: Bool { pendingPrompt != nil || exchanges.contains { $0.isActive } }
    var canSend: Bool { !isActive && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    // MARK: Loading

    /// Loads the conversation when first shown.
    func start() async {
        guard let conversationId, exchanges.isEmpty, let app else { return }
        if let conversation = app.conversation(id: conversationId) {
            sync(with: conversation)
            return
        }
        isLoading = true
        await app.refresh()
        isLoading = false
        if let conversation = app.conversation(id: conversationId) {
            sync(with: conversation)
        } else if app.listError == nil {
            errorMessage = "This conversation no longer exists on the server."
        }
    }

    /// Adds runs this chat hasn't seen (from a refresh of the run list) and replays their events.
    func sync(with conversation: Conversation) {
        guard conversation.id == conversationId else { return }
        for run in conversation.runs where !exchanges.contains(where: { $0.id == run.id }) {
            exchanges.append(Exchange(run: run))
            follow(runId: run.id)
        }
    }

    func stopStreaming() {
        followers.values.forEach { $0.cancel() }
        followers = [:]
    }

    /// Streams a run's events into its exchange until the run ends, reconnecting after
    /// dropped connections (from the last event seen, so nothing is applied twice).
    private func follow(runId: String) {
        guard followers[runId] == nil else { return }
        followers[runId] = Task { [weak self] in
            var failures = 0
            while !Task.isCancelled {
                guard let self, let client = self.app?.client,
                      let after = self.exchange(runId)?.lastSeq
                else { return }
                do {
                    for try await event in client.events(runId: runId, after: after) {
                        self.update(runId) { $0.apply(event) }
                        failures = 0
                    }
                    if self.exchange(runId)?.isActive != true { break }
                    // The stream closed before the run ended: see where the run is.
                    let run = try await client.run(id: runId)
                    if run.status.isTerminal {
                        // It ended in the meantime: read what's left of the log, then take the
                        // run's final state in case the log has no terminal event.
                        let last = self.exchange(runId)?.lastSeq ?? 0
                        for try await event in client.events(runId: runId, after: last) {
                            self.update(runId) { $0.apply(event) }
                        }
                        self.update(runId) { if $0.isActive { $0.run = run } }
                        break
                    }
                } catch is CancellationError {
                    return
                } catch let error as CedarError where error.status == 404 {
                    self.update(runId) {
                        $0.run.status = .failed
                        $0.run.error = "This run no longer exists on the server."
                    }
                    break
                } catch {
                    failures += 1
                    if failures >= 3 { self.errorMessage = error.localizedDescription }
                }
                let delay = min(30.0, pow(2.0, Double(min(failures, 5))))
                try? await Task.sleep(for: .seconds(delay))
            }
            guard let self else { return }
            self.followers[runId] = nil
            if self.exchange(runId)?.isActive == false {
                self.errorMessage = nil
                self.app?.runDidChange()
            }
        }
    }

    private func exchange(_ runId: String) -> Exchange? {
        exchanges.first { $0.id == runId }
    }

    private func update(_ runId: String, _ change: (inout Exchange) -> Void) {
        guard let index = exchanges.firstIndex(where: { $0.id == runId }) else { return }
        change(&exchanges[index])
    }

    // MARK: Sending

    func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard canSend, let app else { return }
        guard let client = app.client else {
            errorMessage = CedarError.notConfigured.errorDescription
            return
        }
        draft = ""
        pendingPrompt = text
        errorMessage = nil
        defer { pendingPrompt = nil }

        let settings = app.settings
        let model = settings.model.isEmpty ? nil : settings.model
        let system = settings.systemPrompt.trimmingCharacters(in: .whitespacesAndNewlines)
        let parent = exchanges.last?.run.id
        do {
            let run: Run
            do {
                run = try await client.createRun(.init(
                    prompt: text,
                    model: model,
                    // A continuation keeps its conversation's system prompt.
                    system: parent == nil && !system.isEmpty ? system : nil,
                    parentId: parent
                ))
            } catch let error as CedarError where error.status == 404 && parent != nil {
                // The previous run is gone from cedar: start over, with the history inlined.
                let history = exchanges.map { (user: $0.prompt, assistant: $0.answer) }
                run = try await client.createRun(.init(
                    prompt: PromptComposer.compose(history: history, message: text),
                    model: model,
                    system: system.isEmpty ? nil : system
                ))
            }
            exchanges.append(Exchange(run: run))
            follow(runId: run.id)
            if conversationId == nil {
                conversationId = run.id
                app.chatDidStart(self, conversationId: run.id)
            } else {
                app.runDidChange()
            }
        } catch {
            draft = text
            errorMessage = error.localizedDescription
            if (error as? CedarError)?.status == 409 {
                // Most likely continued from another device: pick up its runs.
                await app.refresh()
            }
        }
    }

    func stop() async {
        guard let active = exchanges.last(where: { $0.isActive }), let client = app?.client else { return }
        do {
            try await client.cancelRun(id: active.id)
        } catch CedarError.api(409, _, _) {
            // Already finished; the stream will deliver the terminal event.
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}

/// Builds a prompt that carries earlier exchanges, for when the run to continue no longer exists.
/// Mirrors cedar-chat's fallback.
enum PromptComposer {
    static func compose(history: [(user: String, assistant: String)], message: String, maxHistoryBytes: Int = 40_000) -> String {
        guard !history.isEmpty else { return message }
        var kept: [String] = []
        var used = 0
        for exchange in history.reversed() {
            let block = "<user>\n\(exchange.user)\n</user>\n<assistant>\n\(exchange.assistant)\n</assistant>\n"
            if used + block.utf8.count > maxHistoryBytes { break }
            used += block.utf8.count
            kept.append(block)
        }
        var out = """
            This message continues an earlier conversation, shown below for context. Your sandbox has \
            been reset since then, so files and processes from earlier exchanges no longer exist.

            <conversation_history>

            """
        let omitted = history.count - kept.count
        if omitted > 0 { out += "(\(omitted) earlier exchange(s) omitted)\n" }
        out += kept.reversed().joined()
        out += "</conversation_history>\n\nReply to this new message:\n\n"
        out += message
        return out
    }
}
