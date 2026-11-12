package dev.cedar.cedar

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.cedar.cedar.push.PushManager
import dev.cedar.cedar.push.Notifier
import dev.cedar.cedar.storage.SettingsStore
import dev.cedar.cedar.ui.describe
import dev.cedar.core.Agent
import dev.cedar.core.AgentBody
import dev.cedar.core.AgentLimits
import dev.cedar.core.AgentRun
import dev.cedar.core.ChatClient
import dev.cedar.core.ChatException
import dev.cedar.core.Conversation
import dev.cedar.core.Inbox
import dev.cedar.core.Contribution
import dev.cedar.core.ContributionPreview
import dev.cedar.core.ContributionSettings
import dev.cedar.core.MemoryFilter
import dev.cedar.core.MemoryItem
import dev.cedar.core.MemoryLimits
import dev.cedar.core.MemoryProfile
import dev.cedar.core.RecordSet
import dev.cedar.core.RecordSetDetail
import dev.cedar.core.RecordsLimits
import kotlinx.serialization.json.JsonObject
import dev.cedar.core.ModelInfo
import dev.cedar.core.Notice
import dev.cedar.core.ServerSettings
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlin.coroutines.coroutineContext
import kotlinx.coroutines.launch
import java.time.Instant
import kotlin.math.min
import kotlin.math.pow

/** What the list shows: the conversations, the starred ones (chats and agents' runs), or the agents and what they did. */
enum class SidebarMode { CHATS, STARRED, AGENTS }

/** Inside Agents: the agents themselves, or the latest runs of all of them. */
enum class AgentsTab { AGENTS, ACTIVITY }

/**
 * What the agent form opens with: an agent to edit, or a new one, which may come from a finished
 * conversation (`conversationId`, with the prompt the server made of it).
 */
data class AgentFormRequest(
    val agent: Agent? = null,
    val conversationId: String? = null,
    val title: String = "",
    val prompt: String = "",
)

/**
 * App-wide state, as iOS/Cedar/App/AppModel.swift has it: settings, the conversation list, the
 * starred conversations, the agents, the inbox count, and the open chats.
 */
class AppModel(private val context: Context, private val store: SettingsStore, val push: PushManager) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /**
     * The language picked: a tag from `AppLocales.OPTIONS`, "" for Automatic, or null until the user
     * has chosen one (the first step of onboarding).
     */
    var language by mutableStateOf(store.language)
        private set

    /** The locale each message carries and the server is told: the choice, or the device's for Automatic. */
    val locale: String get() = Language.effective(language)

    /** The app's resources in its language, for text made outside an activity. */
    fun text(): Context = Language.localized(context, language)

    /** What went wrong, in the user's language. */
    fun describe(error: Throwable): String = text().describe(error)

    var settings by mutableStateOf(store.load())
        private set
    var conversations by mutableStateOf<List<Conversation>>(emptyList())
        private set
    var models by mutableStateOf<List<ModelInfo>>(emptyList())
        private set

    /** How many finished responses nobody has looked at (the bell in the web UI). */
    var inboxUnread by mutableStateOf(0)
        private set
    var isRefreshing by mutableStateOf(false)
        private set
    var listError by mutableStateOf<String?>(null)
    var sidebarMode by mutableStateOf(SidebarMode.CHATS)

    var agentsTab by mutableStateOf(AgentsTab.AGENTS)

    /** The agents, enabled first, and what the server allows and assumes for them. */
    var agents by mutableStateOf<List<Agent>>(emptyList())
        private set
    var agentLimits by mutableStateOf(AgentLimits())
        private set

    /** The runs of each agent whose page was opened, newest first, once loaded. */
    val agentRuns = mutableStateMapOf<String, List<AgentRun>>()

    /** The agents with a finished run nobody has looked at, and how many runs that is. */
    var unreadAgents by mutableStateOf<Set<String>>(emptySet())
        private set
    var agentsUnread by mutableStateOf(0)
        private set
    var agentsLoaded by mutableStateOf(false)
        private set
    var agentError by mutableStateOf<String?>(null)

    /** The latest runs of every agent, newest first, as far as they were paged in; `activityNext` asks for more. */
    var activity by mutableStateOf<List<AgentRun>>(emptyList())
        private set
    var activityNext by mutableStateOf<String?>(null)
        private set
    var activityLoaded by mutableStateOf(false)
        private set
    var activityLoadingMore by mutableStateOf(false)
        private set
    var activityError by mutableStateOf<String?>(null)

    /** The agent whose page is open (its id), under the conversation that may be open over it. */
    var showingAgent by mutableStateOf<String?>(null)
        private set

    /** The agent form, over everything else, while an agent is being made or edited. */
    var agentForm by mutableStateOf<AgentFormRequest?>(null)

    /**
     * Webhook secrets the server just handed out (on creating the agent, or for a new secret), by
     * agent: shown on the agent's page until it is closed, and kept nowhere.
     */
    val hookSecrets = mutableStateMapOf<String, String>()

    /**
     * Starred conversations, most recently starred first, chats and agents' runs alike. Stars are
     * kept by the server, so they are the same on the web, the desktop app, the iOS app and here.
     */
    var starred by mutableStateOf<List<Conversation>>(emptyList())
        private set
    var starredLoaded by mutableStateOf(false)
        private set
    var starredError by mutableStateOf<String?>(null)

    /**
     * What the list's search field holds. While it holds something, the list shows what the server
     * found for it instead of Chats, Starred or Scheduled.
     */
    var searchText by mutableStateOf("")

    /**
     * The conversations found for `searchQuery`, chats and scheduled runs alike, most recently active
     * first; null until the first answer.
     */
    var searchResults by mutableStateOf<List<Conversation>?>(null)
        private set
    var searchError by mutableStateOf<String?>(null)

    /** What is searched for: `searchText` without the space around it. */
    val searchQuery: String get() = searchText.trim()
    val isSearching: Boolean get() = searchQuery.isNotEmpty()

    /** The server has `/api/memory` (its `/api/config` says so); Settings shows Memory only then. */
    var serverHasMemory by mutableStateOf(false)
        private set

    /**
     * What cedar remembers about the user, oldest first, once loaded (Settings → Memory), as
     * `memoryFilter` says; and the profile it reads before every reply.
     */
    var memoryItems by mutableStateOf<List<MemoryItem>>(emptyList())
        private set
    var memoryProfile by mutableStateOf<MemoryProfile?>(null)
        private set
    /** How many active memories there are of every kind, whatever `memoryFilter` shows: the count on the Settings row. */
    var memoryTotal by mutableStateOf<Int?>(null)
        private set
    var memoryFilter by mutableStateOf(MemoryFilter.ABOUT)
    var memoryLoaded by mutableStateOf(false)
        private set
    var memoryError by mutableStateOf<String?>(null)

    /** False when the chat server's cedar has no memory yet. */
    var memorySupported by mutableStateOf(true)
        private set
    var memoryLimits by mutableStateOf<MemoryLimits?>(null)
        private set
    var showingMemory by mutableStateOf(false)

    /** The server has `/api/records` (its `/api/config` says so); Settings shows Records only then. */
    var serverHasRecords by mutableStateOf(false)
        private set

    /** The sets of records cedar keeps for the user, once loaded (Settings → Records). */
    var recordSets by mutableStateOf<List<RecordSet>>(emptyList())
        private set
    var recordsLoaded by mutableStateOf(false)
        private set
    var recordsError by mutableStateOf<String?>(null)

    /** False when the chat server's cedar has no records yet. */
    var recordsSupported by mutableStateOf(true)
        private set
    var recordsLimits by mutableStateOf<RecordsLimits?>(null)
        private set
    var showingRecords by mutableStateOf(false)
    /** The set whose rows are open, over the Records screen. */
    var showingRecordSet by mutableStateOf<RecordSet?>(null)

    /** The server can send conversations to a corpus (`/api/config` says so); Settings shows Contribute only then. */
    var serverHasContributions by mutableStateOf(false)
        private set
    var showingContribute by mutableStateOf(false)
    var contributionSettings by mutableStateOf<ContributionSettings?>(null)
        private set
    var contributions by mutableStateOf<List<Contribution>>(emptyList())
        private set
    var contributionsError by mutableStateOf<String?>(null)

    /** Conversations the list leaves out (an agent's runs), as their chats loaded them. */
    private val otherConversations = mutableStateMapOf<String, Conversation>()

    /** The selected conversation id, or a draft chat's key. */
    var selection by mutableStateOf<String?>(null)
    var showingSettings by mutableStateOf(!settings.isConfigured)

    /** Whether the feature tour was seen (or skipped); a new install shows it after the language. */
    var tourSeen by mutableStateOf(store.tourSeen)
        private set

    /** The tour, shown again from Settings → About. */
    var showingTour by mutableStateOf(false)

    /** The tour ended. On a new install, Settings opens next so the server can be entered. */
    fun finishTour() {
        store.tourSeen = true
        tourSeen = true
        showingTour = false
        if (!store.isSaved) showingSettings = true
    }

    /**
     * The settings being edited while Settings is open, so that a change of language, which recreates
     * the screen, keeps what was typed. Kept in memory only (it may hold a new access token).
     */
    var settingsDraft: ServerSettings? = null

    /** A `cedar://connect` link opened from outside, for Settings to fill in and test. */
    var pendingConnection by mutableStateOf<dev.cedar.core.ConnectionCode?>(null)

    /** Whether the app is in front; a response that ends while it is not stays unread. */
    var inForeground = true

    /** Chats are kept while the app runs, so a response keeps streaming when you look elsewhere. */
    private val chats = mutableMapOf<String, ChatModel>()
    private var noticesJob: Job? = null
    private var modelsLoaded = false

    val client: ChatClient?
        get() = try {
            ChatClient.from(settings)
        } catch (_: ChatException) {
            null
        }

    // Language

    /**
     * The user picked a language ("" for Automatic), in onboarding or Settings: the app switches to it
     * at once (activities are recreated in it), and the server is told so that agents' runs and
     * notifications follow it, now or once a server is connected.
     */
    fun chooseLanguage(choice: String) {
        adoptLanguage(choice)
        Language.apply(choice)
        Notifier.createChannel(text())
    }

    /** Takes `choice` as the language without applying it (the system already has, on Android 13+). */
    fun adoptLanguage(choice: String) {
        store.language = choice
        language = choice
        store.localePending = true
        syncLocale()
    }

    /** Tells the server the language if it hasn't been told yet. Failures are kept for the next start. */
    private fun syncLocale() {
        val choice = language ?: return
        if (!store.localePending) return
        val client = client ?: return
        scope.launch {
            try {
                client.setLocale(choice.ifEmpty { null })
                if (language == choice) store.localePending = false
            } catch (e: CancellationException) {
                throw e
            } catch (_: Exception) {
                // Not fatal: messages carry the locale anyway, and the next start tries again.
            }
        }
    }

    fun updateSettings(new: ServerSettings) {
        val serverChanged = !new.sameServer(settings)
        val old = settings
        store.save(new)
        settings = new
        if (serverChanged) {
            chats.values.forEach { it.stopStreaming() }
            chats.clear()
            conversations = emptyList()
            otherConversations.clear()
            searchText = ""
            searchResults = null
            searchError = null
            starred = emptyList()
            starredLoaded = false
            agents = emptyList()
            agentLimits = AgentLimits()
            agentRuns.clear()
            unreadAgents = emptySet()
            agentsUnread = 0
            agentsLoaded = false
            agentError = null
            activity = emptyList()
            activityNext = null
            activityLoaded = false
            activityError = null
            showingAgent = null
            agentForm = null
            hookSecrets.clear()
            models = emptyList()
            modelsLoaded = false
            serverHasMemory = false
            serverHasContributions = false
            contributionSettings = null
            contributions = emptyList()
            contributionsError = null
            showingContribute = false
            memoryItems = emptyList()
            memoryProfile = null
            memoryTotal = null
            memoryFilter = MemoryFilter.ABOUT
            memoryLoaded = false
            memoryError = null
            memorySupported = true
            memoryLimits = null
            showingMemory = false
            serverHasRecords = false
            recordSets = emptyList()
            recordsLoaded = false
            recordsError = null
            recordsSupported = true
            recordsLimits = null
            showingRecords = false
            showingRecordSet = null
            selection = null
            inboxUnread = 0
            push.serverChanged(old, new)
            // A newly connected server learns the language too.
            if (language != null && new.isConfigured) store.localePending = true
        }
        start()
    }

    /** Begins following the server: the list, the inbox, and the live notices. */
    fun start() {
        scope.launch { refresh() }
        startNotices()
        syncLocale()
    }

    // Memory

    suspend fun refreshMemory() {
        val client = client ?: return
        try {
            val filter = memoryFilter
            val list = client.memory(filter)
            refreshMemoryTotal()
            // The filter may have changed while the request was out; that refresh is on its way.
            if (filter != memoryFilter) return
            memorySupported = list.supported
            memoryItems = list.items
            memoryProfile = list.profile
            memoryLimits = list.limits
            memoryLoaded = true
            memoryError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            memoryError = describe(e)
        }
    }

    /** Forgets one memory at once (no Save): the next message, in any conversation, goes without it. */
    suspend fun deleteMemory(item: MemoryItem) {
        val client = client ?: throw ChatException.NotConfigured
        try {
            client.deleteMemory(item.id)
        } catch (e: ChatException.Api) {
            // Already gone (another device): the list catches up below.
            if (e.status != 404) throw e
        }
        memoryItems = memoryItems.filter { it.id != item.id }
        refreshMemoryTotal()
    }

    /** Recounts the memories of every kind; a failure keeps the last count. */
    private suspend fun refreshMemoryTotal() {
        val client = client ?: return
        try {
            memoryTotal = client.memoryCount()
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            // The Settings row falls back to the list being shown.
        }
    }

    suspend fun deleteAllMemory() {
        val client = client ?: throw ChatException.NotConfigured
        client.deleteAllMemory()
        memoryItems = emptyList()
        memoryProfile = null
        memoryTotal = 0
    }

    /**
     * Pins, unpins, archives or restores one memory at once. A row that leaves the list being
     * shown (archived from the active list, restored from the archive) is dropped from it.
     */
    suspend fun updateMemory(item: MemoryItem, pinned: Boolean? = null, archived: Boolean? = null) {
        val client = client ?: throw ChatException.NotConfigured
        val updated = client.updateMemory(item.id, pinned = pinned, archived = archived)
        memoryItems = if (updated.isActive == memoryFilter.archived) {
            memoryItems.filter { it.id != item.id }
        } else {
            memoryItems.map { if (it.id == item.id) updated else it }
        }
        if (archived != null) refreshMemoryTotal()
    }

    /** Writes the profile by hand; it then stays as written until changed or cleared here. */
    suspend fun saveMemoryProfile(text: String) {
        val client = client ?: throw ChatException.NotConfigured
        memoryProfile = client.setMemoryProfile(text)
    }

    // Records

    suspend fun refreshRecords() {
        val client = client ?: return
        try {
            val list = client.records()
            recordsSupported = list.supported
            recordSets = list.sets
            recordsLimits = list.limits
            recordsLoaded = true
            recordsError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            recordsError = describe(e)
        }
    }

    /** One set's rows, for its screen. */
    suspend fun loadRecordSet(name: String): RecordSetDetail {
        val client = client ?: throw ChatException.NotConfigured
        return client.recordSet(name)
    }

    /** Deletes one set at once (no Save): the next message, in any conversation, goes without it. */
    suspend fun deleteRecordSet(set: RecordSet) {
        val client = client ?: throw ChatException.NotConfigured
        try {
            client.deleteRecordSet(set.name)
        } catch (e: ChatException.Api) {
            // Already gone (another device): the list catches up below.
            if (e.status != 404) throw e
        }
        recordSets = recordSets.filter { it.id != set.id }
    }

    suspend fun deleteAllRecords() {
        val client = client ?: throw ChatException.NotConfigured
        client.deleteAllRecords()
        recordSets = emptyList()
    }

    // Contributing a conversation

    suspend fun refreshContributions(askCorpus: Boolean = false) {
        val client = client ?: return
        try {
            contributionSettings = client.contributionSettings()
            contributions = if (askCorpus) client.refreshContributions() else client.contributions()
            contributionsError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            contributionsError = describe(e)
        }
    }

    suspend fun updateContributionSettings(patch: JsonObject) {
        val client = client ?: throw ChatException.NotConfigured
        contributionSettings = client.updateContributionSettings(patch)
    }

    suspend fun withdrawContribution(id: String) {
        val client = client ?: throw ChatException.NotConfigured
        val updated = client.withdrawContribution(id)
        contributions = contributions.map { if (it.id == id) updated else it }
    }

    suspend fun contributionPreview(conversationId: String, reasoning: Boolean, fetched: Boolean): ContributionPreview {
        val client = client ?: throw ChatException.NotConfigured
        return client.contributionPreview(conversationId, reasoning, fetched)
    }

    suspend fun sendContribution(conversationId: String, bundle: JsonObject): Contribution {
        val client = client ?: throw ChatException.NotConfigured
        val receipt = client.sendContribution(conversationId, bundle)
        contributions = listOf(receipt) + contributions
        return receipt
    }

    suspend fun rateTurn(turnId: String, rating: Int?) {
        val client = client ?: throw ChatException.NotConfigured
        client.rateTurn(turnId, rating)
    }

    // Conversations

    suspend fun refresh() {
        val client = client ?: run {
            listError = describe(ChatException.NotConfigured)
            return
        }
        isRefreshing = true
        try {
            val list = scope.async { client.conversations() }
            val inbox = scope.async { client.inbox() }
            val stars = scope.async { client.starred() }
            conversations = list.await()
            take(inbox.await())
            listError = null
            try {
                starred = stars.await()
                starredLoaded = true
                starredError = null
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                starredError = describe(e)
            }
            if (!modelsLoaded) {
                modelsLoaded = true
                models = runCatching { client.models() }.getOrDefault(emptyList())
                val config = runCatching { client.config() }.getOrNull()
                serverHasMemory = config?.memory ?: false
                serverHasRecords = config?.records ?: false
                serverHasContributions = config?.contributions ?: false
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            listError = describe(e)
        } finally {
            isRefreshing = false
        }
        if (sidebarMode == SidebarMode.AGENTS || showingAgent != null) refreshAgents()
        if (isSearching) search()
    }

    /**
     * Asks the server for the conversations that match the search field: their titles and everything
     * said in them are searched. An answer that comes after the field changed is dropped.
     */
    suspend fun search() {
        val query = searchQuery
        if (query.isEmpty()) {
            searchResults = null
            searchError = null
            return
        }
        val client = client ?: run {
            searchError = describe(ChatException.NotConfigured)
            return
        }
        try {
            val found = client.search(query)
            if (query != searchQuery) return
            searchResults = found
            searchError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (query == searchQuery) searchError = describe(e)
        }
    }

    private fun take(inbox: Inbox) {
        inboxUnread = inbox.unread
        val ofAgents = inbox.items.mapNotNull { it.scheduleId }
        agentsUnread = ofAgents.size
        unreadAgents = ofAgents.toSet()
    }

    fun conversation(id: String): Conversation? =
        conversations.firstOrNull { it.id == id } ?: otherConversations[id] ?: starred.firstOrNull { it.id == id }
            ?: searchResults?.firstOrNull { it.id == id }

    /** A chat loaded its conversation. One the list leaves out is kept here, so its title shows and opening it marks it seen. */
    fun remember(conversation: Conversation) {
        if (conversations.any { it.id == conversation.id }) return
        val isNew = otherConversations[conversation.id] == null
        otherConversations[conversation.id] = conversation
        if (selection != conversation.id) return
        if (isNew && conversation.scheduleId != null && sidebarMode == SidebarMode.CHATS && !isSearching) {
            // Opened from a push, say: show where it belongs. One opened from Starred or from a search stays there.
            sidebarMode = SidebarMode.AGENTS
            scope.launch { refreshAgents() }
        }
        if (inForeground) markSeen(conversation.id)
    }

    /** Whether a response is running in the conversation, as far as an open chat knows. */
    fun isRunning(conversationId: String): Boolean = chats[conversationId]?.isActive ?: false

    suspend fun rename(conversation: Conversation, title: String) {
        val client = client ?: throw ChatException.NotConfigured
        replace(client.rename(conversation.id, title))
    }

    fun isStarred(conversationId: String): Boolean =
        if (starredLoaded) starred.any { it.id == conversationId } else conversation(conversationId)?.isStarred ?: false

    /**
     * Stars or unstars a conversation (a chat or an agent's run) for every device. The lists change at
     * once and go back if the server refuses.
     */
    suspend fun setStarred(conversationId: String, on: Boolean) {
        val client = client ?: throw ChatException.NotConfigured
        val before = starred
        val known = conversation(conversationId)
        if (on && starred.none { it.id == conversationId } && known != null) {
            starred = listOf(known.copy(starredAt = Instant.now())) + starred
        } else if (!on) {
            starred = starred.filter { it.id != conversationId }
        }
        try {
            replace(client.setStarred(conversationId, on))
            // For the agent's title, and anything starred elsewhere in the meantime.
            runCatching { client.starred() }.getOrNull()?.let { starred = it }
        } catch (e: Exception) {
            starred = before
            throw e
        }
    }

    /** Deletes the conversation and its runs on cedar. The server refuses while a response is running. */
    suspend fun delete(conversation: Conversation) {
        val client = client ?: throw ChatException.NotConfigured
        client.delete(conversation.id)
        chats.remove(conversation.id)?.stopStreaming()
        conversations = conversations.filter { it.id != conversation.id }
        starred = starred.filter { it.id != conversation.id }
        searchResults = searchResults?.filter { it.id != conversation.id }
        if (selection == conversation.id) selection = null
        refresh()
    }

    /** The conversation has been looked at: its responses leave the inbox. */
    fun markSeen(conversationId: String) {
        val client = client ?: return
        val current = conversation(conversationId) ?: return
        if (!current.unread && !current.needsAnswer) return
        scope.launch {
            val updated = runCatching { client.markSeen(conversationId) }.getOrNull() ?: return@launch
            replace(updated)
            runCatching { client.inbox() }.getOrNull()?.let(::take)
            val agentId = updated.scheduleId ?: return@launch
            if (agentRuns.containsKey(agentId)) loadAgent(agentId)
            if (activityLoaded) activity = activity.map { run -> if (run.conversationId == conversationId) run.copy(turn = run.turn?.copy(seen = true)) else run }
        }
    }

    private fun replace(conversation: Conversation) {
        if (conversations.any { it.id == conversation.id }) {
            conversations = conversations.map { if (it.id == conversation.id) conversation else it }
        } else if (otherConversations.containsKey(conversation.id)) {
            otherConversations[conversation.id] = conversation
        }
        // The list's copy carries the agent's title, which other responses don't.
        starred = starred.map { if (it.id == conversation.id) conversation.copy(scheduleTitle = conversation.scheduleTitle ?: it.scheduleTitle) else it }
        // The result's copy carries the passage that matched, too.
        searchResults = searchResults?.map {
            if (it.id == conversation.id) conversation.copy(scheduleTitle = conversation.scheduleTitle ?: it.scheduleTitle, snippet = it.snippet) else it
        }
    }

    // Agents

    /** Reads the agents, and with them whatever else of theirs is on screen: the open agent's runs, the activity. */
    suspend fun refreshAgents() {
        val client = client ?: return
        try {
            val list = client.agents()
            agents = list.agents
            agentLimits = list.limits
            agentsLoaded = true
            agentError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            agentError = describe(e)
            return
        }
        val ids = agents.map { it.id }.toSet()
        agentRuns.keys.filter { it !in ids }.forEach { agentRuns.remove(it) }
        showingAgent?.let { id -> if (id in ids) loadAgent(id) else closeAgent() }
        if (sidebarMode == SidebarMode.AGENTS && agentsTab == AgentsTab.ACTIVITY) refreshActivity()
    }

    fun agent(id: String): Agent? = agents.firstOrNull { it.id == id }

    /** Reads one agent and its runs, for its page. */
    suspend fun loadAgent(id: String) {
        val client = client ?: return
        try {
            val detail = client.agent(id)
            agentRuns[id] = detail.runs
            put(detail.agent)
        } catch (e: CancellationException) {
            throw e
        } catch (e: ChatException.Api) {
            if (e.status == 404) {
                // Deleted elsewhere.
                agents = agents.filter { it.id != id }
                agentRuns.remove(id)
                if (showingAgent == id) closeAgent()
            } else {
                agentError = describe(e)
            }
        } catch (e: Exception) {
            agentError = describe(e)
        }
    }

    /** Takes the server's latest word on an agent: replaces it in the list, or adds it. */
    private fun put(agent: Agent) {
        // An answer without the last run (a new agent's has none to give) keeps the one the list had.
        val known = agents.firstOrNull { it.id == agent.id }
        val fresh = if (agent.lastRun == null && known?.lastRun != null) agent.copy(lastRun = known.lastRun) else agent
        agents = if (known != null) agents.map { if (it.id == agent.id) fresh else it } else listOf(fresh) + agents
        // A secret comes once; the agent's page shows it until it is closed.
        agent.hook?.secret?.let { hookSecrets[agent.id] = it }
    }

    /** Opens an agent's page: what starts it, its prompt and its runs. */
    fun openAgent(id: String) {
        sidebarMode = SidebarMode.AGENTS
        showingAgent = id
        selection = null
        showingSettings = false
        scope.launch {
            if (agent(id) == null) refreshAgents() else loadAgent(id)
        }
    }

    fun closeAgent() {
        showingAgent?.let { hookSecrets.remove(it) }
        showingAgent = null
    }

    /** The first page of every agent's latest runs. */
    suspend fun refreshActivity() {
        val client = client ?: return
        try {
            val page = client.activity()
            val fresh = page.items.map { it.id }.toSet()
            val last = page.items.lastOrNull()?.time
            // Pages read further down stay: only the top is new.
            val older = if (page.nextBefore != null && last != null) activity.filter { it.id !in fresh && (it.time ?: Instant.MAX) <= last } else emptyList()
            activity = page.items + older
            if (older.isEmpty()) activityNext = page.nextBefore
            activityLoaded = true
            activityError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            activityError = describe(e)
        }
    }

    /** The page after the ones shown, when there is one. */
    suspend fun loadMoreActivity() {
        val client = client ?: return
        val before = activityNext ?: return
        if (activityLoadingMore) return
        activityLoadingMore = true
        try {
            val page = client.activity(before = before)
            val known = activity.map { it.id }.toSet()
            activity = activity + page.items.filter { it.id !in known }
            activityNext = page.nextBefore
            activityError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            activityError = describe(e)
        } finally {
            activityLoadingMore = false
        }
    }

    /** Starts a run now and opens it, so it streams in. */
    suspend fun runNow(agent: Agent) {
        val client = client ?: throw ChatException.NotConfigured
        val run = client.runAgent(agent.id)
        loadAgent(agent.id)
        run.conversationId?.let { selection = it }
    }

    /** Pauses or resumes. Resuming skips what was missed while paused. */
    suspend fun setEnabled(agent: Agent, enabled: Boolean) {
        val client = client ?: throw ChatException.NotConfigured
        put(client.updateAgent(agent.id, AgentBody(enabled = enabled)))
    }

    /** Makes the agent, or changes the one with `id`, and reads the list again (it is ordered by the server). */
    suspend fun saveAgent(id: String?, body: AgentBody): Agent {
        val client = client ?: throw ChatException.NotConfigured
        val saved = if (id == null) client.createAgent(body) else client.updateAgent(id, body)
        put(saved)
        refreshAgents()
        return saved
    }

    /** Deletes the agent and its runs. The server refuses while one of them is still going. */
    suspend fun deleteAgent(agent: Agent) {
        val client = client ?: throw ChatException.NotConfigured
        try {
            client.deleteAgent(agent.id)
        } catch (e: ChatException.Api) {
            // Already gone (another device): the list catches up below.
            if (e.status != 404) throw e
        }
        if (showingAgent == agent.id) closeAgent()
        agents = agents.filter { it.id != agent.id }
        val runs = agentRuns.remove(agent.id).orEmpty().mapNotNull { it.conversationId }.toSet()
        if (selection in runs) selection = null
        activity = activity.filter { it.agentId != agent.id }
        refresh()
    }

    /** A new secret for a webhook agent; the old one stops working at once. */
    suspend fun rotateHook(agent: Agent) {
        val client = client ?: throw ChatException.NotConfigured
        put(client.rotateHook(agent.id))
    }

    /**
     * Has the server turn a finished conversation into one prompt, and opens the form for a new
     * agent with it.
     */
    suspend fun makeAgent(conversationId: String) {
        val client = client ?: throw ChatException.NotConfigured
        val compacted = client.compact(conversationId)
        agentForm = AgentFormRequest(
            conversationId = conversationId,
            title = conversation(conversationId)?.title?.trim().orEmpty(),
            prompt = compacted.prompt,
        )
    }

    // Notices

    /** Follows `GET /api/events` while the app is in front, reconnecting with a growing wait. */
    private fun startNotices() {
        noticesJob?.cancel()
        if (client == null) return
        noticesJob = scope.launch {
            var failures = 0
            while (coroutineContext.isActive) {
                val client = client ?: return@launch
                try {
                    client.notices().collect { notice ->
                        failures = 0
                        handle(notice)
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (_: Exception) {
                    failures += 1
                }
                delay((min(60.0, 2.0.pow(min(failures, 6))) * 1000).toLong())
                // Whatever was missed while disconnected is in the list and the inbox.
                refresh()
            }
        }
    }

    private fun handle(notice: Notice) {
        scope.launch { refresh() }
        chats[notice.conversationId]?.noticed(notice)
        if (inForeground && selection == notice.conversationId) {
            // Watched to the end: the web UI marks that seen too.
            scope.launch {
                delay(300)
                markSeen(notice.conversationId)
            }
        }
    }

    /** The app came to the front: whatever happened meanwhile. */
    fun becameActive() {
        inForeground = true
        start()
        selection?.let { id -> if (conversation(id) != null) markSeen(id) }
    }

    /** The app went to the background: Android cuts connections there anyway, so the notices stop until it's back. */
    fun becameInactive() {
        inForeground = false
        noticesJob?.cancel()
        noticesJob = null
    }

    // Chats

    fun newChat() {
        showingAgent?.let { closeAgent() }
        val chat = ChatModel(this)
        chats[chat.key] = chat
        selection = chat.key
    }

    fun chat(id: String): ChatModel = chats.getOrPut(id) { ChatModel(this, conversationId = id) }

    /** Opens a conversation (from a push, say), whether or not the list has it yet. */
    fun open(conversationId: String) {
        selection = conversationId
        showingSettings = false
        scope.launch { refresh() }
    }

    /** Called when a draft chat's conversation is created: from now on it has an id. */
    fun chatDidStart(chat: ChatModel, conversationId: String) {
        chats.remove(chat.key)
        chats[conversationId] = chat
        if (selection == chat.key) selection = conversationId
        scope.launch { refresh() }
    }

    /** A chat's turn ended while it was open and the app in front. */
    fun turnDidEnd(conversationId: String) {
        scope.launch { refresh() }
        if (inForeground && selection == conversationId) markSeen(conversationId)
    }

    /** Whether a notification for `conversationId` would tell the user what is already in front of them. */
    fun isShowing(conversationId: String): Boolean = inForeground && selection == conversationId
}
