import Foundation
import Observation
import SwiftUI

/// What the sidebar lists: the conversations, the starred ones (chats and scheduled runs), or
/// the scheduled agents and their runs.
enum SidebarMode: Hashable {
    case chats, starred, scheduled
}

/// App-wide state: settings, the conversation list, the starred conversations, the scheduled
/// agents, the inbox count, and the open chats.
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
    var sidebarMode: SidebarMode = .chats
    private(set) var schedules: [Schedule] = []
    /// Each expanded schedule's runs, newest first, once loaded.
    private(set) var scheduleRuns: [String: [ScheduleRun]] = [:]
    var expandedSchedules: Set<String> = []
    /// The schedules with a finished run nobody has looked at, and how many runs that is.
    private(set) var unreadSchedules: Set<String> = []
    private(set) var scheduledUnread = 0
    private(set) var schedulesLoaded = false
    var scheduleError: String?
    /// Starred conversations, most recently starred first, chats and scheduled runs alike. Stars
    /// are kept by the server, so they are the same on the web, the desktop app and here.
    private(set) var starred: [Conversation] = []
    private(set) var starredLoaded = false
    var starredError: String?
    /// Conversations the list leaves out (a schedule's runs), as their chats loaded them.
    private var otherConversations: [String: Conversation] = [:]
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
            otherConversations = [:]
            starred = []
            starredLoaded = false
            schedules = []
            scheduleRuns = [:]
            expandedSchedules = []
            unreadSchedules = []
            scheduledUnread = 0
            schedulesLoaded = false
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
            async let stars = client.starred()
            conversations = try await list
            let box = try await inbox
            take(box)
            listError = nil
            do {
                starred = try await stars
                starredLoaded = true
                starredError = nil
            } catch is CancellationError {
            } catch let error as URLError where error.code == .cancelled {
            } catch {
                starredError = error.localizedDescription
            }
            if !modelsLoaded {
                modelsLoaded = true
                models = (try? await client.models()) ?? []
            }
        } catch is CancellationError {
        } catch {
            listError = error.localizedDescription
        }
        if sidebarMode == .scheduled { await refreshSchedules() }
    }

    private func take(_ inbox: Inbox) {
        inboxUnread = inbox.unread
        let scheduled = inbox.items.compactMap(\.scheduleId)
        scheduledUnread = scheduled.count
        unreadSchedules = Set(scheduled)
    }

    func conversation(id: String) -> Conversation? {
        conversations.first { $0.id == id } ?? otherConversations[id] ?? starred.first { $0.id == id }
    }

    /// A chat loaded its conversation. One the list leaves out is kept here, so its title shows
    /// and opening it marks it seen.
    func remember(_ conversation: Conversation) {
        guard !conversations.contains(where: { $0.id == conversation.id }) else { return }
        let isNew = otherConversations[conversation.id] == nil
        otherConversations[conversation.id] = conversation
        guard selection == conversation.id else { return }
        if isNew, let scheduleId = conversation.scheduleId, sidebarMode != .starred,
           sidebarMode != .scheduled || !expandedSchedules.contains(scheduleId) {
            // Opened from a push, say: show where it belongs. One opened from Starred stays there.
            sidebarMode = .scheduled
            expandedSchedules.insert(scheduleId)
            Task { await refreshSchedules() }
        }
        if isActive { markSeen(conversationId: conversation.id) }
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

    func isStarred(conversationId: String) -> Bool {
        if starredLoaded { return starred.contains { $0.id == conversationId } }
        return conversation(id: conversationId)?.isStarred ?? false
    }

    /// Stars or unstars a conversation (a chat or a scheduled run) for every device. The lists
    /// change at once and go back if the server refuses.
    func setStarred(conversationId: String, _ on: Bool) async throws {
        guard let client else { throw ChatError.notConfigured }
        let before = starred
        if on, !starred.contains(where: { $0.id == conversationId }), var known = conversation(id: conversationId) {
            known.starredAt = .now
            starred.insert(known, at: 0)
        } else if !on {
            starred.removeAll { $0.id == conversationId }
        }
        do {
            let updated = try await client.setStarred(conversationId: conversationId, starred: on)
            replace(updated)
            // For the schedule's title, and anything starred elsewhere in the meantime.
            if let fresh = try? await client.starred() { starred = fresh }
        } catch {
            starred = before
            throw error
        }
    }

    /// Deletes the conversation and its runs on cedar. The server refuses while a response is
    /// running.
    func delete(_ conversation: Conversation) async throws {
        guard let client else { throw ChatError.notConfigured }
        try await client.delete(conversationId: conversation.id)
        chats.removeValue(forKey: conversation.id)?.stopStreaming()
        conversations.removeAll { $0.id == conversation.id }
        starred.removeAll { $0.id == conversation.id }
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
                if let inbox = try? await client.inbox() { take(inbox) }
                if let scheduleId = updated.scheduleId, scheduleRuns[scheduleId] != nil {
                    await loadRuns(scheduleId: scheduleId)
                }
            }
        }
    }

    private func replace(_ conversation: Conversation) {
        if let index = conversations.firstIndex(where: { $0.id == conversation.id }) {
            conversations[index] = conversation
        } else if otherConversations[conversation.id] != nil {
            otherConversations[conversation.id] = conversation
        }
        if let index = starred.firstIndex(where: { $0.id == conversation.id }) {
            // The list's copy carries the schedule's title, which other responses don't.
            var updated = conversation
            updated.scheduleTitle = updated.scheduleTitle ?? starred[index].scheduleTitle
            starred[index] = updated
        }
    }

    // MARK: Schedules

    /// Reads the schedules, and the runs of the ones that are expanded.
    func refreshSchedules() async {
        guard let client else { return }
        do {
            schedules = try await client.schedules()
            schedulesLoaded = true
            scheduleError = nil
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            scheduleError = error.localizedDescription
            return
        }
        let ids = Set(schedules.map(\.id))
        scheduleRuns = scheduleRuns.filter { ids.contains($0.key) }
        expandedSchedules.formIntersection(ids)
        for id in expandedSchedules {
            await loadRuns(scheduleId: id)
        }
    }

    func loadRuns(scheduleId: String) async {
        guard let client else { return }
        do {
            let detail = try await client.schedule(id: scheduleId)
            scheduleRuns[scheduleId] = detail.runs
            if let index = schedules.firstIndex(where: { $0.id == scheduleId }) {
                var schedule = detail.schedule
                schedule.lastRun = detail.runs.first
                schedules[index] = schedule
            }
        } catch let error as ChatError where error.status == 404 {
            schedules.removeAll { $0.id == scheduleId }
            scheduleRuns.removeValue(forKey: scheduleId)
            expandedSchedules.remove(scheduleId)
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            scheduleError = error.localizedDescription
        }
    }

    /// Starts a run now and opens it, so it streams in.
    func runNow(_ schedule: Schedule) async throws {
        guard let client else { throw ChatError.notConfigured }
        let run = try await client.runSchedule(id: schedule.id)
        expandedSchedules.insert(schedule.id)
        await loadRuns(scheduleId: schedule.id)
        if let conversationId = run.conversationId {
            selection = conversationId
            compactColumn = .detail
        }
    }

    func setEnabled(_ schedule: Schedule, _ enabled: Bool) async throws {
        guard let client else { throw ChatError.notConfigured }
        var updated = try await client.setSchedule(id: schedule.id, enabled: enabled)
        if let index = schedules.firstIndex(where: { $0.id == schedule.id }) {
            updated.lastRun = schedules[index].lastRun
            schedules[index] = updated
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
