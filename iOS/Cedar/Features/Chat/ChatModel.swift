import Foundation
import Observation
import PhotosUI
import SwiftUI
import UniformTypeIdentifiers

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
    /// The files that went with it.
    private(set) var pendingAttachments: [AttachmentInfo] = []
    private(set) var isLoading = false
    var errorMessage: String?
    var draft = ""
    /// Files picked to go with the next message.
    private(set) var attachments: [OutgoingAttachment] = []
    /// Picked files still being read.
    private(set) var isAddingAttachments = false

    @ObservationIgnored private weak var app: AppModel?
    @ObservationIgnored private var followers: [String: Task<Void, Never>] = [:]
    /// The first load, owned here rather than by the view: SwiftUI can rebuild a chat's view
    /// while it loads, and the rebuilt view joins this load instead of cancelling it.
    @ObservationIgnored private var loading: Task<Void, Never>?

    init(app: AppModel, conversationId: String? = nil) {
        self.app = app
        self.conversationId = conversationId
    }

    var client: ChatClient? { app?.client }
    var isActive: Bool { pendingPrompt != nil || turns.contains { $0.isActive } }
    var canSend: Bool {
        !isActive && !isAddingAttachments && !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// Whether the model new messages go to takes images: the one picked in Settings, or the
    /// server's default. PDFs go with any model.
    var acceptsImages: Bool {
        guard let app else { return false }
        let model = app.settings.model.isEmpty
            ? app.models.first(where: \.isDefault)
            : app.models.first(where: { $0.id == app.settings.model })
        return model?.multimodal ?? false
    }

    /// The questions the last turn asked, until a later turn answers them.
    var openQuestions: Questions? {
        guard let last = turns.last, !last.isActive, last.turn.needsAnswer else { return nil }
        return last.questions
    }

    // MARK: Loading

    /// Loads the conversation when first shown.
    func start() async {
        guard conversationId != nil, turns.isEmpty else { return }
        if loading == nil {
            isLoading = true
            loading = Task { [weak self] in
                guard let self else { return }
                await self.reload()
                self.isLoading = false
                self.loading = nil
            }
        }
        await loading?.value
    }

    /// Reads the conversation from the server and follows any turn not seen yet.
    func reload() async {
        guard let conversationId, let client else { return }
        do {
            let detail = try await client.conversation(id: conversationId)
            app?.remember(detail.conversation)
            sync(with: detail.turns)
            errorMessage = nil
        } catch let error as ChatError where error.status == 404 {
            errorMessage = L10n.string("This conversation no longer exists on the server.")
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
            // URLSession's cancellation, not a failure: its description is just "cancelled".
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
                        $0.turn.error = L10n.string("This response no longer exists on the server.")
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

    // MARK: Attachments

    /// Reads one picked file: its name, type and bytes, or nil when there's nothing to read.
    private typealias FileReader = () async throws -> (name: String, type: UTType?, data: Data)?

    /// Adds photos picked from the library.
    func addPhotos(_ items: [PhotosPickerItem]) async {
        await addFiles(items.enumerated().map { index, item -> FileReader in
            {
                guard let data = try await item.loadTransferable(type: Data.self) else { return nil }
                let type = item.supportedContentTypes.first { $0.conforms(to: .image) } ?? .image
                let ext = type.preferredFilenameExtension ?? "jpg"
                return (items.count == 1 ? "photo.\(ext)" : "photo-\(index + 1).\(ext)", type, data)
            }
        })
    }

    /// Adds files picked in the Files app.
    func addFiles(at urls: [URL]) async {
        await addFiles(urls.map { url -> FileReader in
            {
                let scoped = url.startAccessingSecurityScopedResource()
                defer { if scoped { url.stopAccessingSecurityScopedResource() } }
                let data = try Data(contentsOf: url)
                return (url.lastPathComponent, UTType(filenameExtension: url.pathExtension), data)
            }
        })
    }

    /// Reads each file and adds the ones that fit, reporting the first that doesn't.
    private func addFiles(_ readers: [FileReader]) async {
        isAddingAttachments = true
        defer { isAddingAttachments = false }
        var problems: [String] = []
        for read in readers {
            do {
                guard let picked = try await read() else {
                    problems.append(L10n.string("A picked file could not be read"))
                    continue
                }
                let (name, type, data) = picked
                // Re-encoding a photo takes a moment; keep it off the main thread.
                let file = await Task.detached { OutgoingAttachment.make(name: name, type: type, data: data) }.value
                guard let file else {
                    problems.append(L10n.format("%@: only PNG, JPEG, GIF, WebP and PDF files can be attached", name))
                    continue
                }
                if let problem = OutgoingAttachment.problem(adding: file, to: attachments, imagesAllowed: acceptsImages) {
                    problems.append(problem)
                    continue
                }
                attachments.append(file)
            } catch {
                problems.append(error.localizedDescription)
            }
        }
        if let problem = problems.first { errorMessage = problem }
    }

    func removeAttachment(_ file: OutgoingAttachment) {
        attachments.removeAll { $0.id == file.id }
    }

    // MARK: Sending

    func send() async {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard canSend else { return }
        let files = attachments
        draft = ""
        attachments = []
        if !(await send(text: text, attachments: files)) {
            draft = text
            attachments = files
        }
    }

    /// Sends `text`, with `attachments`, as the next message. Returns whether the server took it.
    @discardableResult
    func send(text: String, attachments: [OutgoingAttachment] = []) async -> Bool {
        guard let app, !isActive else { return false }
        guard let client = app.client else {
            errorMessage = ChatError.notConfigured.errorDescription
            return false
        }
        pendingPrompt = text
        pendingAttachments = attachments.map(\.info)
        errorMessage = nil
        defer {
            pendingPrompt = nil
            pendingAttachments = []
        }
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
                reasoning: settings.reasoning.isEmpty ? nil : settings.reasoning,
                attachments: attachments.isEmpty ? nil : attachments.map {
                    .init(name: $0.name, mediaType: $0.mediaType, data: $0.data.base64EncodedString())
                },
                plan: settings.plan.isEmpty ? nil : settings.plan,
                // The reply comes in the interface language.
                locale: LanguageStore.shared.effectiveTag
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
