package dev.cedar.cedar

import android.content.Context
import android.net.Uri
import android.util.Base64
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.cedar.cedar.ui.Attachments
import dev.cedar.cedar.ui.attachmentProblem
import dev.cedar.core.Approval
import dev.cedar.core.AttachmentInfo
import dev.cedar.core.ChatClient
import dev.cedar.core.ChatException
import dev.cedar.core.ClarifyAnswer
import dev.cedar.core.Notice
import dev.cedar.core.OutgoingAttachment
import dev.cedar.core.Plan
import dev.cedar.core.Questions
import dev.cedar.core.TranscriptItem
import dev.cedar.core.Turn
import dev.cedar.core.TurnStatus
import dev.cedar.core.TurnTranscript
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.util.UUID
import kotlin.math.min
import kotlin.math.pow

/** One conversation's turns, kept up to date from the server's event streams (iOS/Cedar/Features/Chat/ChatModel.swift). */
class ChatModel(private val app: AppModel, conversationId: String? = null) {
    /** Stable identity, also used as the selection key while the chat is still a draft. */
    val key: String = UUID.randomUUID().toString()
    var conversationId by mutableStateOf(conversationId)
        private set
    var turns by mutableStateOf<List<TurnTranscript>>(emptyList())
        private set

    /** The message being sent, shown until the server accepts it, and the files that went with it. */
    var pendingPrompt by mutableStateOf<String?>(null)
        private set
    var pendingAttachments by mutableStateOf<List<AttachmentInfo>>(emptyList())
        private set
    var isLoading by mutableStateOf(false)
        private set
    var errorMessage by mutableStateOf<String?>(null)
    var draft by mutableStateOf("")

    /** Files picked to go with the next message, and whether picked files are still being read. */
    var attachments by mutableStateOf<List<OutgoingAttachment>>(emptyList())
        private set
    var isAddingAttachments by mutableStateOf(false)
        private set

    private val followers = mutableMapOf<String, Job>()
    private var loading: Job? = null

    val client: ChatClient? get() = app.client
    val isActive: Boolean get() = pendingPrompt != null || turns.any { it.isActive }
    val canSend: Boolean get() = !isActive && !isAddingAttachments && draft.isNotBlank()

    /** Whether the model new messages go to takes images: the one picked in Settings, or the server's default. PDFs go with any model. */
    val acceptsImages: Boolean
        get() {
            val model = if (app.settings.model.isEmpty()) app.models.firstOrNull { it.isDefault } else app.models.firstOrNull { it.id == app.settings.model }
            return model?.multimodal ?: false
        }

    /** The questions the last turn asked, until a later turn answers them. */
    val openQuestions: Questions?
        get() {
            val last = turns.lastOrNull() ?: return null
            if (last.isActive || !last.turn.needsAnswer) return null
            return last.questions
        }

    // Loading

    /** Loads the conversation when first shown. */
    fun start() {
        if (conversationId == null || turns.isNotEmpty() || loading != null) return
        isLoading = true
        loading = app.scope.launch {
            reload()
            isLoading = false
            loading = null
        }
    }

    /** Reads the conversation from the server and follows any turn not seen yet. */
    suspend fun reload() {
        val id = conversationId ?: return
        val client = client ?: return
        try {
            val detail = client.conversation(id)
            app.remember(detail.conversation)
            sync(detail.turns)
            errorMessage = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: ChatException.Api) {
            errorMessage = if (e.status == 404) app.text().getString(R.string.conversation_gone) else app.describe(e)
        } catch (e: Exception) {
            errorMessage = app.describe(e)
        }
    }

    /** Adds turns this chat hasn't seen and replays their events; refreshes the ones it has, in case a stream was missed. */
    private fun sync(fresh: List<Turn>) {
        for (turn in fresh.sortedBy { it.seq }) {
            val existing = transcript(turn.id)
            if (existing != null) {
                if (existing.isActive && turn.status.isTerminal) {
                    // It ended without the stream saying so: read what's left, then take the server's final state.
                    follow(turn.id)
                    update(turn.id) { it.copy(turn = turn) }
                }
            } else {
                turns = turns + TurnTranscript(turn)
                follow(turn.id)
            }
        }
    }

    /** A response ended somewhere: if here, make sure its stream was read to the end. */
    fun noticed(notice: Notice) {
        if (notice.conversationId != conversationId) return
        val existing = transcript(notice.turnId)
        if (existing != null && existing.isActive && followers[notice.turnId] == null) {
            follow(notice.turnId)
        } else if (existing == null) {
            // Sent from another device: pick it up.
            app.scope.launch { reload() }
        }
    }

    fun stopStreaming() {
        followers.values.forEach { it.cancel() }
        followers.clear()
    }

    /**
     * Streams a turn's events into its transcript until the run ends, reconnecting after dropped
     * connections (from the last event seen, so nothing is applied twice).
     */
    private fun follow(turnId: String) {
        if (followers[turnId] != null) return
        followers[turnId] = app.scope.launch {
            var failures = 0
            while (true) {
                val client = client ?: break
                val after = transcript(turnId)?.lastSeq ?: break
                try {
                    client.turnEvents(turnId, after).collect { event ->
                        update(turnId) { it.apply(event) }
                        failures = 0
                    }
                    if (transcript(turnId)?.isActive != true) break
                    // The stream closed before the run ended: see where the turn is.
                    val id = conversationId
                    if (id != null) {
                        val turn = client.conversation(id).turns.firstOrNull { it.id == turnId }
                        if (turn != null && turn.status.isTerminal) {
                            val last = transcript(turnId)?.lastSeq ?: 0
                            client.turnEvents(turnId, last).collect { event -> update(turnId) { it.apply(event) } }
                            update(turnId) { if (it.isActive) it.copy(turn = turn) else it }
                            break
                        }
                    }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: ChatException.Api) {
                    if (e.status == 404) {
                        val gone = app.text().getString(R.string.response_gone)
                        update(turnId) { it.copy(turn = it.turn.copy(status = TurnStatus.FAILED, error = gone)) }
                        break
                    }
                    failures += 1
                    if (failures >= 3) errorMessage = app.describe(e)
                } catch (e: Exception) {
                    failures += 1
                    if (failures >= 3) errorMessage = app.describe(e)
                }
                delay((min(30.0, 2.0.pow(min(failures, 5))) * 1000).toLong())
            }
            followers.remove(turnId)
            val id = conversationId
            if (transcript(turnId)?.isActive == false && id != null) {
                errorMessage = null
                app.turnDidEnd(id)
            }
        }
    }

    private fun transcript(turnId: String): TurnTranscript? = turns.firstOrNull { it.id == turnId }

    private fun update(turnId: String, change: (TurnTranscript) -> TurnTranscript) {
        turns = turns.map { if (it.id == turnId) change(it) else it }
    }

    // Attachments

    /** Reads picked files (photos or documents) and adds the ones that fit, reporting the first that doesn't. */
    fun addFiles(context: Context, uris: List<Uri>) {
        if (uris.isEmpty()) return
        isAddingAttachments = true
        app.scope.launch {
            val problems = mutableListOf<String>()
            for ((index, uri) in uris.withIndex()) {
                // Reading and re-encoding a photo takes a moment; keep it off the main thread.
                val read = withContext(Dispatchers.IO) { runCatching { Attachments.read(context, uri, index, uris.size) } }
                if (read.isFailure) {
                    problems += read.exceptionOrNull()?.message ?: context.getString(R.string.file_unreadable)
                    continue
                }
                val file = read.getOrNull()
                if (file == null) {
                    val name = withContext(Dispatchers.IO) { runCatching { Attachments.name(context, uri) }.getOrNull() }
                    problems += context.getString(R.string.file_type_unsupported, name ?: context.getString(R.string.a_file))
                    continue
                }
                val problem = OutgoingAttachment.check(file, attachments, acceptsImages)
                if (problem != null) {
                    problems += context.attachmentProblem(problem)
                    continue
                }
                attachments = attachments + file
            }
            isAddingAttachments = false
            problems.firstOrNull()?.let { errorMessage = it }
        }
    }

    fun removeAttachment(file: OutgoingAttachment) {
        attachments = attachments.filter { it.id != file.id }
    }

    // Sending

    fun send() {
        val text = draft.trim()
        if (!canSend) return
        val files = attachments
        draft = ""
        attachments = emptyList()
        app.scope.launch {
            if (!send(text, files)) {
                draft = text
                attachments = files
            }
        }
    }

    /** Sends `text`, with `files`, as the next message. Returns whether the server took it. */
    suspend fun send(text: String, files: List<OutgoingAttachment> = emptyList()): Boolean {
        if (isActive) return false
        val client = app.client ?: run {
            errorMessage = app.describe(ChatException.NotConfigured)
            return false
        }
        pendingPrompt = text
        pendingAttachments = files.map { it.info }
        errorMessage = null
        val settings = app.settings
        try {
            val id = conversationId ?: client.createConversation().id.also {
                conversationId = it
                app.chatDidStart(this, it)
            }
            val turn = client.send(id, ChatClient.SendMessage(
                content = text,
                model = settings.model.ifEmpty { null },
                reasoning = settings.reasoning.ifEmpty { null },
                plan = settings.plan.ifEmpty { null },
                // The agent answers, and formats, in the user's locale.
                locale = app.locale,
                attachments = files.takeIf { it.isNotEmpty() }?.map {
                    ChatClient.SendMessage.Upload(it.name, it.mediaType, Base64.encodeToString(it.data, Base64.NO_WRAP))
                },
            ))
            turns = turns + TurnTranscript(turn)
            follow(turn.id)
            app.scope.launch { app.refresh() }
            return true
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            errorMessage = app.describe(e)
            if ((e as? ChatException.Api)?.status == 409) {
                // A response is running, most likely started from another device: show it.
                reload()
            }
            return false
        } finally {
            pendingPrompt = null
            pendingAttachments = emptyList()
        }
    }

    /** Answers the open clarifying questions. */
    fun answer(questions: Questions, answers: List<ClarifyAnswer.Answer>) {
        val text = app.text()
        val answer = ClarifyAnswer.text(
            questions, answers,
            other = { text.getString(R.string.clarify_other, it) },
            noPreference = text.getString(R.string.clarify_no_preference),
        )
        app.scope.launch { send(answer) }
    }

    fun stop() {
        val active = turns.lastOrNull { it.isActive } ?: return
        val client = client ?: return
        app.scope.launch {
            try {
                client.cancel(active.id)
            } catch (e: ChatException.Api) {
                // 409: already finished; the stream will deliver the terminal event.
                if (e.status != 409) errorMessage = app.describe(e)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                errorMessage = app.describe(e)
            }
        }
    }

    // Plans and approvals

    fun decide(approval: Approval, turn: TurnTranscript, decision: String) {
        val client = client ?: return
        app.scope.launch {
            try {
                client.decideApproval(turn.id, approval.id, decision)
                // The stream brings `approval.resolved`; until then, show the choice.
                update(turn.id) { t ->
                    t.copy(items = t.items.map { item ->
                        if (item is TranscriptItem.ApprovalItem && item.approval.id == approval.id && item.approval.isPending) {
                            TranscriptItem.ApprovalItem(item.approval.copy(decision = decision, by = "user"))
                        } else item
                    })
                }
            } catch (e: ChatException.Api) {
                // 409: no longer waiting; the stream has, or will have, the outcome.
                if (e.status != 409) errorMessage = app.describe(e)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                errorMessage = app.describe(e)
            }
        }
    }

    fun agree(plan: Plan, turn: TurnTranscript) {
        val client = client ?: return
        app.scope.launch {
            try {
                client.answerPlan(turn.id, plan.steps)
            } catch (e: ChatException.Api) {
                if (e.status != 409) errorMessage = app.describe(e)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                errorMessage = app.describe(e)
            }
        }
    }
}
