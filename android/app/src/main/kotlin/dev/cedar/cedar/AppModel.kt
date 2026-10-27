package dev.cedar.cedar

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.cedar.cedar.push.PushManager
import dev.cedar.cedar.push.Notifier
import dev.cedar.cedar.storage.SettingsStore
import dev.cedar.cedar.ui.describe
import dev.cedar.core.ChatClient
import dev.cedar.core.ChatException
import dev.cedar.core.Conversation
import dev.cedar.core.Inbox
import dev.cedar.core.ModelInfo
import dev.cedar.core.Notice
import dev.cedar.core.Schedule
import dev.cedar.core.ScheduleRun
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

/** What the list shows: the conversations, the starred ones (chats and scheduled runs), or the scheduled agents and their runs. */
enum class SidebarMode { CHATS, STARRED, SCHEDULED }

/**
 * App-wide state, as iOS/Cedar/App/AppModel.swift has it: settings, the conversation list, the
 * starred conversations, the scheduled agents, the inbox count, and the open chats.
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

    var schedules by mutableStateOf<List<Schedule>>(emptyList())
        private set

    /** Each expanded schedule's runs, newest first, once loaded. */
    val scheduleRuns = mutableStateMapOf<String, List<ScheduleRun>>()
    val expandedSchedules = mutableStateListOf<String>()

    /** The schedules with a finished run nobody has looked at, and how many runs that is. */
    var unreadSchedules by mutableStateOf<Set<String>>(emptySet())
        private set
    var scheduledUnread by mutableStateOf(0)
        private set
    var schedulesLoaded by mutableStateOf(false)
        private set
    var scheduleError by mutableStateOf<String?>(null)

    /**
     * Starred conversations, most recently starred first, chats and scheduled runs alike. Stars are
     * kept by the server, so they are the same on the web, the desktop app, the iOS app and here.
     */
    var starred by mutableStateOf<List<Conversation>>(emptyList())
        private set
    var starredLoaded by mutableStateOf(false)
        private set
    var starredError by mutableStateOf<String?>(null)

    /** Conversations the list leaves out (a schedule's runs), as their chats loaded them. */
    private val otherConversations = mutableStateMapOf<String, Conversation>()

    /** The selected conversation id, or a draft chat's key. */
    var selection by mutableStateOf<String?>(null)
    var showingSettings by mutableStateOf(!settings.isConfigured)

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
     * at once (activities are recreated in it), and the server is told so that scheduled runs and
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
            starred = emptyList()
            starredLoaded = false
            schedules = emptyList()
            scheduleRuns.clear()
            expandedSchedules.clear()
            unreadSchedules = emptySet()
            scheduledUnread = 0
            schedulesLoaded = false
            models = emptyList()
            modelsLoaded = false
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
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            listError = describe(e)
        } finally {
            isRefreshing = false
        }
        if (sidebarMode == SidebarMode.SCHEDULED) refreshSchedules()
    }

    private fun take(inbox: Inbox) {
        inboxUnread = inbox.unread
        val scheduled = inbox.items.mapNotNull { it.scheduleId }
        scheduledUnread = scheduled.size
        unreadSchedules = scheduled.toSet()
    }

    fun conversation(id: String): Conversation? =
        conversations.firstOrNull { it.id == id } ?: otherConversations[id] ?: starred.firstOrNull { it.id == id }

    /** A chat loaded its conversation. One the list leaves out is kept here, so its title shows and opening it marks it seen. */
    fun remember(conversation: Conversation) {
        if (conversations.any { it.id == conversation.id }) return
        val isNew = otherConversations[conversation.id] == null
        otherConversations[conversation.id] = conversation
        if (selection != conversation.id) return
        val scheduleId = conversation.scheduleId
        if (isNew && scheduleId != null && sidebarMode != SidebarMode.STARRED &&
            (sidebarMode != SidebarMode.SCHEDULED || scheduleId !in expandedSchedules)
        ) {
            // Opened from a push, say: show where it belongs. One opened from Starred stays there.
            sidebarMode = SidebarMode.SCHEDULED
            if (scheduleId !in expandedSchedules) expandedSchedules += scheduleId
            scope.launch { refreshSchedules() }
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
     * Stars or unstars a conversation (a chat or a scheduled run) for every device. The lists change at
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
            // For the schedule's title, and anything starred elsewhere in the meantime.
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
            val scheduleId = updated.scheduleId
            if (scheduleId != null && scheduleRuns.containsKey(scheduleId)) loadRuns(scheduleId)
        }
    }

    private fun replace(conversation: Conversation) {
        if (conversations.any { it.id == conversation.id }) {
            conversations = conversations.map { if (it.id == conversation.id) conversation else it }
        } else if (otherConversations.containsKey(conversation.id)) {
            otherConversations[conversation.id] = conversation
        }
        // The list's copy carries the schedule's title, which other responses don't.
        starred = starred.map { if (it.id == conversation.id) conversation.copy(scheduleTitle = conversation.scheduleTitle ?: it.scheduleTitle) else it }
    }

    // Schedules

    /** Reads the schedules, and the runs of the ones that are expanded. */
    suspend fun refreshSchedules() {
        val client = client ?: return
        try {
            schedules = client.schedules()
            schedulesLoaded = true
            scheduleError = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            scheduleError = describe(e)
            return
        }
        val ids = schedules.map { it.id }.toSet()
        scheduleRuns.keys.filter { it !in ids }.forEach { scheduleRuns.remove(it) }
        expandedSchedules.retainAll(ids)
        for (id in expandedSchedules.toList()) loadRuns(id)
    }

    suspend fun loadRuns(scheduleId: String) {
        val client = client ?: return
        try {
            val detail = client.schedule(scheduleId)
            scheduleRuns[scheduleId] = detail.runs
            schedules = schedules.map { if (it.id == scheduleId) detail.schedule.copy(lastRun = detail.runs.firstOrNull()) else it }
        } catch (e: CancellationException) {
            throw e
        } catch (e: ChatException.Api) {
            if (e.status == 404) {
                schedules = schedules.filter { it.id != scheduleId }
                scheduleRuns.remove(scheduleId)
                expandedSchedules.remove(scheduleId)
            } else {
                scheduleError = describe(e)
            }
        } catch (e: Exception) {
            scheduleError = describe(e)
        }
    }

    fun toggleExpanded(schedule: Schedule) {
        if (schedule.id in expandedSchedules) {
            expandedSchedules.remove(schedule.id)
        } else {
            expandedSchedules += schedule.id
            scope.launch { loadRuns(schedule.id) }
        }
    }

    /** Starts a run now and opens it, so it streams in. */
    suspend fun runNow(schedule: Schedule) {
        val client = client ?: throw ChatException.NotConfigured
        val run = client.runSchedule(schedule.id)
        if (schedule.id !in expandedSchedules) expandedSchedules += schedule.id
        loadRuns(schedule.id)
        run.conversationId?.let { selection = it }
    }

    suspend fun setEnabled(schedule: Schedule, enabled: Boolean) {
        val client = client ?: throw ChatException.NotConfigured
        val updated = client.setSchedule(schedule.id, enabled)
        schedules = schedules.map { if (it.id == schedule.id) updated.copy(lastRun = it.lastRun) else it }
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
