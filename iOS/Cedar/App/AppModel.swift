import Foundation
import Observation
import SwiftUI

/// App-wide state: settings, the conversation list, and the open chats.
@MainActor
@Observable
final class AppModel {
    private(set) var settings: ServerSettings
    private(set) var conversations: [Conversation] = []
    private(set) var titles: [String: String]
    private(set) var isRefreshing = false
    var listError: String?
    /// The selected conversation id, or a draft chat's key.
    var selection: String?
    /// Which column a compact (iPhone) split view shows.
    var compactColumn: NavigationSplitViewColumn = .sidebar
    var showingSettings = false

    /// Chats are kept while the app runs, so a response keeps streaming when you look elsewhere.
    /// Not observed: it is filled lazily while views are built.
    @ObservationIgnored private var chats: [String: ChatModel] = [:]
    private let titleStore = TitleStore()

    init() {
        settings = ServerSettings.load()
        titles = TitleStore().all
        showingSettings = !settings.isConfigured
    }

    var client: CedarClient? { try? CedarClient(settings: settings) }

    func updateSettings(_ new: ServerSettings) {
        let serverChanged = new.serverURL != settings.serverURL || new.apiKey != settings.apiKey
        new.save()
        settings = new
        if serverChanged {
            chats.values.forEach { $0.stopStreaming() }
            chats = [:]
            conversations = []
            selection = nil
        }
        Task { await refresh() }
    }

    // MARK: Conversations

    func refresh() async {
        guard let client else {
            listError = CedarError.notConfigured.errorDescription
            return
        }
        isRefreshing = true
        defer { isRefreshing = false }
        do {
            conversations = Conversation.group(try await client.allRuns())
            listError = nil
            for conversation in conversations {
                chats[conversation.id]?.sync(with: conversation)
            }
        } catch is CancellationError {
        } catch {
            listError = error.localizedDescription
        }
    }

    func title(for conversation: Conversation) -> String {
        titles[conversation.id] ?? conversation.defaultTitle
    }

    func conversation(id: String) -> Conversation? {
        conversations.first { $0.id == id }
    }

    func rename(_ conversation: Conversation, to title: String) {
        titleStore.set(title, for: conversation.id)
        titles = titleStore.all
    }

    /// Deletes every run of the conversation on cedar, newest first, which also frees its
    /// workspace. cedar refuses while a run is active.
    func delete(_ conversation: Conversation) async throws {
        guard let client else { throw CedarError.notConfigured }
        for run in conversation.runs.reversed() {
            do {
                try await client.deleteRun(id: run.id)
            } catch let error as CedarError where error.status == 404 {
                continue
            }
        }
        chats.removeValue(forKey: conversation.id)?.stopStreaming()
        titleStore.set(nil, for: conversation.id)
        titles = titleStore.all
        conversations.removeAll { $0.id == conversation.id }
        if selection == conversation.id {
            selection = nil
            compactColumn = .sidebar
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

    /// Called when a draft chat's first run is created: from now on it is a conversation.
    func chatDidStart(_ chat: ChatModel, conversationId: String) {
        chats.removeValue(forKey: chat.key)
        chats[conversationId] = chat
        if selection == chat.key { selection = conversationId }
        Task { await refresh() }
    }

    func runDidChange() {
        Task { await refresh() }
    }
}
