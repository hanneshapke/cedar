import Foundation
import Observation
import SwiftUI

/// What the sidebar lists: the conversations, the starred ones (chats and agents' runs), or the
/// agents and what they did.
enum SidebarMode: Hashable {
    case chats, starred, agents
}

/// What the sidebar's Agents list shows: the agents, or the latest runs of all of them.
enum AgentsTab: Hashable {
    case agents, activity
}

/// App-wide state: settings, the conversation list, the starred conversations, the agents, the
/// inbox count, and the open chats.
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
    var agentsTab: AgentsTab = .agents
    /// The agents, enabled first, as the server lists them.
    private(set) var agents: [Agent] = []
    /// The server's limits and the defaults an agent gets when it sets none.
    private(set) var agentLimits = AgentList.Limits()
    /// An agent's runs, newest first, once its detail loaded them.
    private(set) var agentRuns: [String: [AgentRun]] = [:]
    /// The agents with a finished run nobody has looked at, and how many runs that is.
    private(set) var unreadAgents: Set<String> = []
    private(set) var agentsUnread = 0
    private(set) var agentsLoaded = false
    var agentError: String?
    /// A webhook agent's secret, which the server tells once: when the agent is made or the
    /// secret replaced. Kept until the agent's detail has shown it.
    private(set) var agentSecrets: [String: String] = [:]
    /// The latest runs of every agent, newest first, as far as they are loaded; `activityNext`
    /// asks for the page after them.
    private(set) var activity: [ActivityItem] = []
    private(set) var activityNext: String?
    private(set) var activityLoaded = false
    private(set) var isLoadingActivity = false
    var activityError: String?
    /// The agent each conversation that is a run belongs to, as its chat loaded it.
    private(set) var conversationAgents: [String: AgentRef] = [:]
    /// Starred conversations, most recently starred first, chats and agents' runs alike. Stars
    /// are kept by the server, so they are the same on the web, the desktop app and here.
    private(set) var starred: [Conversation] = []
    private(set) var starredLoaded = false
    var starredError: String?
    /// The server has `/api/memory` (its `/api/config` says so); Settings shows Memory only then.
    private(set) var serverHasMemory = false
    /// What cedar remembers about the user, oldest first, once loaded (Settings → Memory), as
    /// `memoryFilter` says; and the profile it reads before every reply.
    private(set) var memoryItems: [MemoryItem] = []
    private(set) var memoryProfile: MemoryProfile?
    /// How many active memories there are of every kind, whatever `memoryFilter` shows: the
    /// count on the Settings row.
    private(set) var memoryTotal: Int?
    var memoryFilter: MemoryFilter = .about
    private(set) var memoryLoaded = false
    var memoryError: String?
    /// False when the chat server's cedar has no memory yet.
    private(set) var memorySupported = true
    private(set) var memoryLimits: MemoryList.Limits?
    /// The server has `/api/records` (its `/api/config` says so); Settings shows Records only then.
    private(set) var serverHasRecords = false
    /// The sets of records cedar keeps for the user, once loaded (Settings → Records).
    private(set) var recordSets: [RecordSet] = []
    private(set) var recordsLoaded = false
    var recordsError: String?
    /// False when the chat server's cedar has no records yet.
    private(set) var recordsSupported = true
    private(set) var recordsLimits: RecordsList.Limits?
    /// The server can send conversations to a corpus (`/api/config` says so); Settings shows Contribute only then.
    private(set) var serverHasContributions = false
    private(set) var contributionSettings: ContributionSettings?
    private(set) var contributions: [Contribution] = []
    var contributionsError: String?
    /// Conversations the list leaves out (an agent's runs), as their chats loaded them.
    private var otherConversations: [String: Conversation] = [:]
    /// The selected conversation id, or a draft chat's key.
    var selection: String?
    /// Which column a compact (iPhone) split view shows.
    var compactColumn: NavigationSplitViewColumn = .sidebar
    var showingSettings = false
    /// Whether the feature tour was seen (or skipped); a new install shows it after the language.
    private(set) var tourSeen: Bool
    /// Whether the app is in front; a response that ends while it is not stays unread.
    var isActive = true

    /// Chats are kept while the app runs, so a response keeps streaming when you look elsewhere.
    /// Not observed: it is filled lazily while views are built.
    @ObservationIgnored private var chats: [String: ChatModel] = [:]
    @ObservationIgnored private var noticesTask: Task<Void, Never>?
    @ObservationIgnored private var modelsLoaded = false

    init() {
        settings = ServerSettings.load()
        tourSeen = UserDefaults.standard.bool(forKey: "tourSeen")
        showingSettings = !settings.isConfigured
    }

    /// The tour ended. On a new install, Settings opens next so the server can be entered.
    func finishTour() {
        tourSeen = true
        UserDefaults.standard.set(true, forKey: "tourSeen")
        if !ServerSettings.isSaved() { showingSettings = true }
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
            agents = []
            agentLimits = AgentList.Limits()
            agentRuns = [:]
            unreadAgents = []
            agentsUnread = 0
            agentsLoaded = false
            agentError = nil
            agentSecrets = [:]
            agentsTab = .agents
            activity = []
            activityNext = nil
            activityLoaded = false
            activityError = nil
            conversationAgents = [:]
            models = []
            modelsLoaded = false
            serverHasMemory = false
            serverHasContributions = false
            contributionSettings = nil
            contributions = []
            contributionsError = nil
            memoryItems = []
            memoryProfile = nil
            memoryTotal = nil
            memoryFilter = .about
            memoryLoaded = false
            memoryError = nil
            memorySupported = true
            memoryLimits = nil
            serverHasRecords = false
            recordSets = []
            recordsLoaded = false
            recordsError = nil
            recordsSupported = true
            recordsLimits = nil
            selection = nil
            inboxUnread = 0
            PushRegistrar.shared?.serverChanged(from: old, to: new)
            // A new server doesn't know the language yet.
            if LanguageStore.shared.hasChosen { localePending = true }
        }
        start()
    }

    /// Begins following the server: the list, the inbox, and the live notices.
    func start() {
        Task { await refresh() }
        startNotices()
        syncLocale()
    }

    // MARK: Language

    /// Picks the interface language (`nil` follows the device), on first launch or in Settings,
    /// and tells the server.
    func chooseLanguage(_ tag: String?) {
        let first = !LanguageStore.shared.hasChosen
        LanguageStore.shared.choose(tag)
        localePending = true
        syncLocale()
        // Asking for notifications waited for the language to be picked.
        if first, settings.isConfigured {
            Task { await PushRegistrar.shared?.requestAuthorization() }
        }
    }

    /// Whether the server has yet to hear the language picked, kept across launches.
    private var localePending: Bool {
        get { UserDefaults.standard.bool(forKey: "localePending") }
        set { UserDefaults.standard.set(newValue, forKey: "localePending") }
    }

    /// Sends the picked language to the server (`PUT /api/settings/locale`), for its agents' runs
    /// and notifications. Not fatal when it fails: it is sent again on the next launch or change of
    /// server, and messages carry the language anyway.
    private func syncLocale() {
        guard localePending, LanguageStore.shared.hasChosen, let client else { return }
        let tag = LanguageStore.shared.chosenTag
        Task {
            do {
                try await client.setLocale(tag)
                // A later pick sends its own.
                if LanguageStore.shared.chosenTag == tag { localePending = false }
            } catch {
                print("could not send the language: \(error.localizedDescription)")
            }
        }
    }

    // MARK: Memory

    func refreshMemory() async {
        guard let client else { return }
        do {
            let filter = memoryFilter
            async let total = try? client.memoryCount()
            let list = try await client.memory(filter)
            memoryTotal = await total ?? nil
            // The filter may have changed while the request was out; that refresh is on its way.
            guard filter == memoryFilter else { return }
            memorySupported = list.supported
            memoryItems = list.items
            memoryProfile = list.profile
            memoryLimits = list.limits
            memoryLoaded = true
            memoryError = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            memoryError = error.localizedDescription
        }
    }

    /// Forgets one memory at once (no Save): the next message, in any conversation, goes without it.
    func deleteMemory(_ item: MemoryItem) async throws {
        guard let client else { throw ChatError.notConfigured }
        do {
            try await client.deleteMemory(id: item.id)
        } catch let error as ChatError where error.status == 404 {
            // Already gone (another device).
        }
        memoryItems.removeAll { $0.id == item.id }
        await refreshMemoryTotal()
    }

    /// Recounts the memories of every kind after one was removed, archived or restored.
    private func refreshMemoryTotal() async {
        guard let client else { return }
        if let total = try? await client.memoryCount() { memoryTotal = total }
    }

    func deleteAllMemory() async throws {
        guard let client else { throw ChatError.notConfigured }
        try await client.deleteAllMemory()
        memoryItems = []
        memoryProfile = nil
        memoryTotal = 0
    }

    /// Pins, unpins, archives or restores one memory at once. A row that leaves the list being
    /// shown (archived from the active list, restored from the archive) is dropped from it.
    func updateMemory(_ item: MemoryItem, pinned: Bool? = nil, archived: Bool? = nil) async throws {
        guard let client else { throw ChatError.notConfigured }
        let updated = try await client.updateMemory(id: item.id, MemoryChange(pinned: pinned, archived: archived))
        if updated.isActive == memoryFilter.archived {
            memoryItems.removeAll { $0.id == item.id }
        } else if let i = memoryItems.firstIndex(where: { $0.id == item.id }) {
            memoryItems[i] = updated
        }
        if archived != nil { await refreshMemoryTotal() }
    }

    /// Writes the profile by hand; it then stays as written until changed or cleared here.
    func saveMemoryProfile(_ text: String) async throws {
        guard let client else { throw ChatError.notConfigured }
        memoryProfile = try await client.setMemoryProfile(text)
    }

    // MARK: Records

    func refreshRecords() async {
        guard let client else { return }
        do {
            let list = try await client.records()
            recordsSupported = list.supported
            recordSets = list.sets
            recordsLimits = list.limits
            recordsLoaded = true
            recordsError = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            recordsError = error.localizedDescription
        }
    }

    /// One set's rows, for its screen.
    func loadRecordSet(_ name: String) async throws -> RecordSetDetail {
        guard let client else { throw ChatError.notConfigured }
        return try await client.recordSet(name: name)
    }

    /// Deletes one set at once (no Save): the next message, in any conversation, goes without it.
    func deleteRecordSet(_ set: RecordSet) async throws {
        guard let client else { throw ChatError.notConfigured }
        do {
            try await client.deleteRecordSet(name: set.name)
        } catch let error as ChatError where error.status == 404 {
            // Already gone (another device).
        }
        recordSets.removeAll { $0.id == set.id }
    }

    func deleteAllRecords() async throws {
        guard let client else { throw ChatError.notConfigured }
        try await client.deleteAllRecords()
        recordSets = []
    }

    // MARK: Contributing a conversation

    func refreshContributions(askCorpus: Bool = false) async {
        guard let client else { return }
        do {
            contributionSettings = try await client.contributionSettings()
            contributions = try await (askCorpus ? client.refreshContributions() : client.contributions())
            contributionsError = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            contributionsError = error.localizedDescription
        }
    }

    func updateContributionSettings(_ patch: ContributionSettingsPatch) async throws {
        guard let client else { throw ChatError.notConfigured }
        contributionSettings = try await client.updateContributionSettings(patch)
    }

    func withdrawContribution(id: String) async throws {
        guard let client else { throw ChatError.notConfigured }
        let updated = try await client.withdrawContribution(id: id)
        contributions = contributions.map { $0.id == id ? updated : $0 }
    }

    func contributionPreview(conversationId: String, reasoning: Bool, fetched: Bool) async throws -> ContributionPreview {
        guard let client else { throw ChatError.notConfigured }
        return try await client.contributionPreview(conversationId: conversationId, reasoning: reasoning, fetched: fetched)
    }

    func sendContribution(conversationId: String, bundle: JSONValue) async throws -> Contribution {
        guard let client else { throw ChatError.notConfigured }
        let receipt = try await client.sendContribution(conversationId: conversationId, bundle: bundle)
        contributions.insert(receipt, at: 0)
        return receipt
    }

    func rateTurn(turnId: String, rating: Int?) async throws {
        guard let client else { throw ChatError.notConfigured }
        _ = try await client.rateTurn(turnId: turnId, rating: rating)
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
                let config = try? await client.config()
                serverHasMemory = config?.memory ?? false
                serverHasRecords = config?.records ?? false
                serverHasContributions = config?.contributions ?? false
            }
        } catch is CancellationError {
        } catch {
            listError = error.localizedDescription
        }
        if sidebarMode == .agents { await refreshAgentsList() }
    }

    private func take(_ inbox: Inbox) {
        inboxUnread = inbox.unread
        let runs = inbox.items.compactMap(\.scheduleId)
        agentsUnread = runs.count
        unreadAgents = Set(runs)
    }

    func conversation(id: String) -> Conversation? {
        conversations.first { $0.id == id } ?? otherConversations[id] ?? starred.first { $0.id == id }
    }

    /// A chat loaded its conversation. One the list leaves out is kept here, so its title shows
    /// and opening it marks it seen; `agent` is the agent whose run it is.
    func remember(_ conversation: Conversation, agent: AgentRef? = nil) {
        if let agent, conversationAgents[conversation.id] != agent {
            conversationAgents[conversation.id] = agent
        }
        guard !conversations.contains(where: { $0.id == conversation.id }) else { return }
        let isNew = otherConversations[conversation.id] == nil
        otherConversations[conversation.id] = conversation
        guard selection == conversation.id else { return }
        if isNew, conversation.scheduleId != nil, sidebarMode == .chats {
            // Opened from a push, say: show where it belongs. One opened from Starred stays there.
            sidebarMode = .agents
            Task { await refreshAgentsList() }
        }
        if isActive { markSeen(conversationId: conversation.id) }
    }

    /// The agent whose run a conversation is, for the badge in its chat: as the server named
    /// it, or from what the lists know on a server that doesn't.
    func agent(ofConversation id: String) -> AgentRef? {
        if let known = conversationAgents[id] { return known }
        guard let conversation = conversation(id: id), let agentId = conversation.scheduleId else { return nil }
        let title = agents.first { $0.id == agentId }?.title ?? conversation.scheduleTitle ?? L10n.string("Agent")
        return AgentRef(id: agentId, title: title)
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

    /// Stars or unstars a conversation (a chat or an agent's run) for every device. The lists
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
            // For the agent's title, and anything starred elsewhere in the meantime.
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
                if let agentId = updated.scheduleId {
                    if agentRuns[agentId] != nil { await loadAgent(id: agentId) }
                    if sidebarMode == .agents { await refreshAgentsList() }
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
            // The list's copy carries the agent's title, which other responses don't.
            var updated = conversation
            updated.scheduleTitle = updated.scheduleTitle ?? starred[index].scheduleTitle
            starred[index] = updated
        }
    }

    // MARK: Agents

    /// Reads what the sidebar's Agents list shows: the agents, and the activity when that is up.
    func refreshAgentsList() async {
        await refreshAgents()
        if agentsTab == .activity { await refreshActivity() }
    }

    func refreshAgents() async {
        guard let client else { return }
        do {
            let list = try await client.agents()
            agents = list.agents
            agentLimits = list.limits
            agentsLoaded = true
            agentError = nil
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch let error as ChatError where error.status == 404 {
            agentError = L10n.string("This server is from before agents. Update cedar-chat to use them.")
            return
        } catch {
            agentError = error.localizedDescription
            return
        }
        let ids = Set(agents.map(\.id))
        agentRuns = agentRuns.filter { ids.contains($0.key) }
        agentSecrets = agentSecrets.filter { ids.contains($0.key) }
    }

    /// Reads one agent and its runs, for its detail.
    func loadAgent(id: String) async {
        guard let client else { return }
        do {
            let detail = try await client.agent(id: id)
            agentRuns[id] = detail.runs
            take(detail.agent)
        } catch let error as ChatError where error.status == 404 {
            agents.removeAll { $0.id == id }
            agentRuns.removeValue(forKey: id)
            agentSecrets.removeValue(forKey: id)
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            agentError = error.localizedDescription
        }
    }

    /// An agent as the server just answered it: it replaces the list's copy (or joins the
    /// list), and a secret that came with it is kept for its detail to show.
    private func take(_ agent: Agent) {
        if let secret = agent.hook?.secret, !secret.isEmpty {
            agentSecrets[agent.id] = secret
        }
        if let index = agents.firstIndex(where: { $0.id == agent.id }) {
            agents[index] = agent
        } else {
            agents.append(agent)
        }
    }

    /// The agent's detail has shown the secret; it isn't kept any longer.
    func forgetSecret(agentId: String) {
        agentSecrets.removeValue(forKey: agentId)
    }

    /// Starts a run now and opens it, so it streams in.
    func runNow(_ agent: Agent) async throws {
        guard let client else { throw ChatError.notConfigured }
        let run = try await client.runAgent(id: agent.id)
        await loadAgent(id: agent.id)
        if agentsTab == .activity { await refreshActivity() }
        if let conversationId = run.conversationId {
            selection = conversationId
            compactColumn = .detail
        }
    }

    func setEnabled(_ agent: Agent, _ enabled: Bool) async throws {
        guard let client else { throw ChatError.notConfigured }
        take(try await client.updateAgent(id: agent.id, AgentBody(enabled: enabled)))
    }

    @discardableResult
    func createAgent(_ body: AgentBody) async throws -> Agent {
        guard let client else { throw ChatError.notConfigured }
        let agent = try await client.createAgent(body)
        take(agent)
        agentsLoaded = true
        return agent
    }

    @discardableResult
    func updateAgent(id: String, _ body: AgentBody) async throws -> Agent {
        guard let client else { throw ChatError.notConfigured }
        let agent = try await client.updateAgent(id: id, body)
        take(agent)
        return agent
    }

    /// Deletes the agent. The server refuses while one of its runs is still going.
    func deleteAgent(_ agent: Agent) async throws {
        guard let client else { throw ChatError.notConfigured }
        do {
            try await client.deleteAgent(id: agent.id)
        } catch let error as ChatError where error.status == 404 {
            // Already gone (another device).
        }
        agents.removeAll { $0.id == agent.id }
        agentRuns.removeValue(forKey: agent.id)
        agentSecrets.removeValue(forKey: agent.id)
        activity.removeAll { $0.run.scheduleId == agent.id }
    }

    /// Replaces a webhook agent's secret; the old one stops working at once.
    func rotateHook(_ agent: Agent) async throws {
        guard let client else { throw ChatError.notConfigured }
        take(try await client.rotateHook(id: agent.id))
    }

    /// Tries a watching trigger out once, with nothing kept.
    func testTrigger(_ trigger: Trigger) async throws -> TriggerTest {
        guard let client else { throw ChatError.notConfigured }
        return try await client.testTrigger(trigger)
    }

    /// Reads the newest page of the activity feed.
    func refreshActivity() async {
        guard let client else { return }
        do {
            let page = try await client.activity()
            activity = page.items
            activityNext = page.nextBefore
            activityLoaded = true
            activityError = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch let error as ChatError where error.status == 404 {
            activityError = L10n.string("This server is from before agents. Update cedar-chat to use them.")
        } catch {
            activityError = error.localizedDescription
        }
    }

    /// Reads the page after the runs already loaded.
    func loadMoreActivity() async {
        guard let client, let before = activityNext, !isLoadingActivity else { return }
        isLoadingActivity = true
        defer { isLoadingActivity = false }
        do {
            let page = try await client.activity(before: before)
            let known = Set(activity.map(\.id))
            activity.append(contentsOf: page.items.filter { !known.contains($0.id) })
            activityNext = page.nextBefore
            activityError = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            activityError = error.localizedDescription
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
