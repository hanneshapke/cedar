import Foundation
import Observation
import SwiftUI

/// App-wide state: settings, the conversation list, the inbox count, and the open chats.
@MainActor
@Observable
final class AppModel {
    private(set) var settings: ServerSettings
    private(set) var conversations: [Conversation] = []
    private(set) var models: [ModelInfo] = []
    /// How many finished responses nobody has looked at (the bell in the web UI).
    private(set) var inboxUnread = 0
    private(set) var isRefreshing = false
    var listError: String?
    /// The selected conversation id, or a draft chat's key.
    var selection: String?
    /// Which column a compact (iPhone) split view shows.
    var compactColumn: NavigationSplitViewColumn = .sidebar
    var showingSettings = false
    /// Whether the app is in front; a response that ends while it is not stays unread.
    var isActive = true

    /// Chats are kept while the app runs, so a response keeps streaming when you look elsewhere.
    /// Not observed: it is filled lazily while views are built.
    @ObservationIgnored private var chats: [String: ChatModel] = [:]
    @ObservationIgnored private var noticesTask: Task<Void, Never>?
    @ObservationIgnored private var modelsLoaded = false

    init() {
        settings = ServerSettings.load()
        showingSettings = !settings.isConfigured
    }

    var client: ChatClient? { try? ChatClient(settings: settings) }

    func updateSettings(_ new: ServerSettings) {
        let serverChanged = !new.sameServer(as: settings)
        let old = settings
        new.save()
        settings = new
        if serverChanged {
            chats.values.forEach { $0.stopStreaming() }
            chats = [:]
            conversations = []
            models = []
            modelsLoaded = false
            selection = nil
            inboxUnread = 0
            PushRegistrar.shared?.serverChanged(from: old, to: new)
        }
        start()
    }

    /// Begins following the server: the list, the inbox, and the live notices.
    func start() {
        Task { await refresh() }
        startNotices()
    }

    // MARK: Conversations

    func refresh() async {
        guard let client else {
            listError = ChatError.notConfigured.errorDescription
            return
        }
        isRefreshing = true
        defer { isRefreshing = false }
        do {
            async let list = client.conversations()
            async let inbox = client.inbox()
            conversations = try await list
            let box = try await inbox
            inboxUnread = box.unread
            listError = nil
            if !modelsLoaded {
                modelsLoaded = true
                models = (try? await client.models()) ?? []
            }
        } catch is CancellationError {
        } catch {
            listError = error.localizedDescription
        }
    }

    func conversation(id: String) -> Conversation? {
        conversations.first { $0.id == id }
    }

    func title(for conversation: Conversation) -> String {
        conversation.displayTitle
    }

    /// Whether a response is running in the conversation, as far as an open chat knows.
    func isActive(conversationId: String) -> Bool {
        chats[conversationId]?.isActive ?? false
    }

    func rename(_ conversation: Conversation, to title: String) async throws {
        guard let client else { throw ChatError.notConfigured }
        let updated = try await client.rename(conversationId: conversation.id, title: title)
        replace(updated)
    }

    /// Deletes the conversation and its runs on cedar. The server refuses while a response is
    /// running.
    func delete(_ conversation: Conversation) async throws {
        guard let client else { throw ChatError.notConfigured }
        try await client.delete(conversationId: conversation.id)
        chats.removeValue(forKey: conversation.id)?.stopStreaming()
        conversations.removeAll { $0.id == conversation.id }
        if selection == conversation.id {
            selection = nil
            compactColumn = .sidebar
        }
        await refresh()
    }

    /// The conversation has been looked at: its responses leave the inbox.
    func markSeen(conversationId: String) {
        guard let client, let current = conversation(id: conversationId), current.unread || current.needsAnswer else { return }
        Task {
            if let updated = try? await client.markSeen(conversationId: conversationId) {
                replace(updated)
                inboxUnread = (try? await client.inbox().unread) ?? inboxUnread
            }
        }
    }

    private func replace(_ conversation: Conversation) {
        if let index = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[index] = conversation
        }
    }

    // MARK: Notices

    /// Follows `GET /api/events` for as long as the app runs, reconnecting with a growing wait.
    private func startNotices() {
        noticesTask?.cancel()
        guard client != nil else { return }
        noticesTask = Task { [weak self] in
            var failures = 0
            while !Task.isCancelled {
                guard let self, let client = self.client else { return }
                do {
                    for try await notice in client.notices() {
                        failures = 0
                        self.handle(notice)
                    }
                } catch is CancellationError {
                    return
                } catch {
                    failures += 1
                }
                let delay = min(60.0, pow(2.0, Double(min(failures, 6))))
                try? await Task.sleep(for: .seconds(delay))
                // Whatever was missed while disconnected is in the list and the inbox.
                await self.refresh()
            }
        }
    }

    private func handle(_ notice: Notice) {
        Task { await refresh() }
        chats[notice.conversationId]?.noticed(notice)
        if isActive && selection == notice.conversationId {
            // Watched to the end: the web UI marks that seen too.
            Task {
                try? await Task.sleep(for: .milliseconds(300))
                self.markSeen(conversationId: notice.conversationId)
            }
        }
    }

    /// The app came to the front: whatever happened meanwhile.
    func becameActive() {
        isActive = true
        Task { await refresh() }
        if let selection, conversation(id: selection) != nil {
            markSeen(conversationId: selection)
        }
    }

    // MARK: Chats

    func newChat() {
        let chat = ChatModel(app: self)
        chats[chat.key] = chat
        selection = chat.key
        compactColumn = .detail
    }

    func chat(for id: String) -> ChatModel {
        if let chat = chats[id] { return chat }
        let chat = ChatModel(app: self, conversationId: id)
        chats[id] = chat
        return chat
    }

    /// Opens a conversation (from a push, say), whether or not the list has it yet.
    func open(conversationId: String) {
        selection = conversationId
        compactColumn = .detail
        showingSettings = false
        Task { await refresh() }
    }

    /// Called when a draft chat's conversation is created: from now on it has an id.
    func chatDidStart(_ chat: ChatModel, conversationId: String) {
        chats.removeValue(forKey: chat.key)
        chats[conversationId] = chat
        if selection == chat.key { selection = conversationId }
        Task { await refresh() }
    }

    /// A chat's turn ended while it was open and the app in front.
    func turnDidEnd(conversationId: String) {
        Task { await refresh() }
        if isActive && selection == conversationId {
            markSeen(conversationId: conversationId)
        }
    }
}
