import Foundation
import Observation

/// One conversation's turns, kept up to date from the server's event streams.
@MainActor
@Observable
final class ChatModel {
    /// Stable identity, also used as the selection key while the chat is still a draft.
    let key = UUID().uuidString
    private(set) var conversationId: String?
    private(set) var turns: [TurnTranscript] = []
    /// The message being sent, shown until the server accepts it.
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

    var client: ChatClient? { app?.client }
    var isActive: Bool { pendingPrompt != nil || turns.contains { $0.isActive } }
    var canSend: Bool { !isActive && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// The questions the last turn asked, until a later turn answers them.
    var openQuestions: Questions? {
        guard let last = turns.last, !last.isActive, last.turn.needsAnswer else { return nil }
        return last.questions
    }

    // MARK: Loading

    /// Loads the conversation when first shown.
    func start() async {
        guard conversationId != nil, turns.isEmpty else { return }
        isLoading = true
        await reload()
        isLoading = false
    }

    /// Reads the conversation from the server and follows any turn not seen yet.
    func reload() async {
        guard let conversationId, let client else { return }
        do {
            let detail = try await client.conversation(id: conversationId)
            sync(with: detail.turns)
            errorMessage = nil
        } catch let error as ChatError where error.status == 404 {
            errorMessage = "This conversation no longer exists on the server."
        } catch is CancellationError {
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// Adds turns this chat hasn't seen and replays their events; refreshes the state of the
    /// ones it has, in case a stream was missed.
    private func sync(with fresh: [Turn]) {
        for turn in fresh.sorted(by: { $0.seq < $1.seq }) {
            if let index = turns.firstIndex(where: { $0.id == turn.id }) {
                if turns[index].isActive && turn.status.isTerminal {
                    // It ended without the stream saying so: read what's left, then take the
                    // server's final state.
                    follow(turnId: turn.id)
                    turns[index].turn = turn
                }
            } else {
                turns.append(TurnTranscript(turn: turn))
                follow(turnId: turn.id)
            }
        }
    }

    /// A response ended somewhere: if here, make sure its stream was read to the end.
    func noticed(_ notice: Notice) {
        guard notice.conversationId == conversationId else { return }
        if let index = turns.firstIndex(where: { $0.id == notice.turnId }), turns[index].isActive, followers[notice.turnId] == nil {
            follow(turnId: notice.turnId)
        } else if !turns.contains(where: { $0.id == notice.turnId }) {
            // Sent from another device: pick it up.
            Task { await reload() }
        }
    }

    func stopStreaming() {
        followers.values.forEach { $0.cancel() }
        followers = [:]
    }

    /// Streams a turn's events into its transcript until the run ends, reconnecting after
    /// dropped connections (from the last event seen, so nothing is applied twice).
    private func follow(turnId: String) {
        guard followers[turnId] == nil else { return }
        followers[turnId] = Task { [weak self] in
            var failures = 0
            while !Task.isCancelled {
                guard let self, let client = self.client, let after = self.transcript(turnId)?.lastSeq else { return }
                do {
                    for try await event in client.turnEvents(turnId: turnId, after: after) {
                        self.update(turnId) { $0.apply(event) }
                        failures = 0
                    }
                    if self.transcript(turnId)?.isActive != true { break }
                    // The stream closed before the run ended: see where the turn is.
                    if let conversationId = self.conversationId {
                        let detail = try await client.conversation(id: conversationId)
                        if let turn = detail.turns.first(where: { $0.id == turnId }), turn.status.isTerminal {
                            let last = self.transcript(turnId)?.lastSeq ?? 0
                            for try await event in client.turnEvents(turnId: turnId, after: last) {
                                self.update(turnId) { $0.apply(event) }
                            }
                            self.update(turnId) { if $0.isActive { $0.turn = turn } }
                            break
                        }
                    }
                } catch is CancellationError {
                    return
                } catch let error as ChatError where error.status == 404 {
                    self.update(turnId) {
                        $0.turn.status = .failed
                        $0.turn.error = "This response no longer exists on the server."
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
            self.followers[turnId] = nil
            if self.transcript(turnId)?.isActive == false, let conversationId = self.conversationId {
                self.errorMessage = nil
                self.app?.turnDidEnd(conversationId: conversationId)
            }
        }
    }

    private func transcript(_ turnId: String) -> TurnTranscript? {
        turns.first { $0.id == turnId }
    }

    private func update(_ turnId: String, _ change: (inout TurnTranscript) -> Void) {
        guard let index = turns.firstIndex(where: { $0.id == turnId }) else { return }
        change(&turns[index])
    }

    // MARK: Sending

    func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard canSend else { return }
        draft = ""
        if !(await send(text: text)) {
            draft = text
        }
    }

    /// Sends `text` as the next message. Returns whether the server took it.
    @discardableResult
    func send(text: String) async -> Bool {
        guard let app, !isActive else { return false }
        guard let client = app.client else {
            errorMessage = ChatError.notConfigured.errorDescription
            return false
        }
        pendingPrompt = text
        errorMessage = nil
        defer { pendingPrompt = nil }
        let settings = app.settings
        do {
            let id: String
            if let conversationId {
                id = conversationId
            } else {
                id = try await client.createConversation().id
                conversationId = id
                app.chatDidStart(self, conversationId: id)
            }
            let turn = try await client.send(conversationId: id, .init(
                content: text,
                model: settings.model.isEmpty ? nil : settings.model,
                reasoning: settings.reasoning.isEmpty ? nil : settings.reasoning
            ))
            turns.append(TurnTranscript(turn: turn))
            follow(turnId: turn.id)
            Task { await app.refresh() }
            return true
        } catch {
            errorMessage = error.localizedDescription
            if (error as? ChatError)?.status == 409 {
                // A response is running, most likely started from another device: show it.
                await reload()
            }
            return false
        }
    }

    /// Answers the open clarifying questions.
    func answer(_ questions: Questions, with answers: [ClarifyAnswer.Answer]) async {
        await send(text: ClarifyAnswer.text(for: questions, answers: answers))
    }

    func stop() async {
        guard let active = turns.last(where: { $0.isActive }), let client else { return }
        do {
            try await client.cancel(turnId: active.id)
        } catch ChatError.api(409, _, _) {
            // Already finished; the stream will deliver the terminal event.
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    // MARK: Plans and approvals

    func decide(_ approval: Approval, in turn: TurnTranscript, decision: String) async {
        guard let client else { return }
        do {
            try await client.decideApproval(turnId: turn.id, approvalId: approval.id, decision: decision)
            // The stream brings `approval.resolved`; until then, show the choice.
            update(turn.id) { transcript in
                if let index = transcript.items.firstIndex(where: { $0.id == "approval-\(approval.id)" }),
                   case .approval(var a) = transcript.items[index], a.isPending {
                    a.decision = decision
                    a.by = "user"
                    transcript.items[index] = .approval(a)
                }
            }
        } catch ChatError.api(409, _, _) {
            // No longer waiting: the stream has, or will have, the outcome.
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    func agree(to plan: Plan, in turn: TurnTranscript) async {
        guard let client else { return }
        do {
            try await client.answerPlan(turnId: turn.id, steps: plan.steps)
        } catch ChatError.api(409, _, _) {
        } catch {
            errorMessage = error.localizedDescription
        }
    }
}
