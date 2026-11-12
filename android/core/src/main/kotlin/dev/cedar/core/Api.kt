@file:UseSerializers(InstantSerializer::class)

package dev.cedar.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.time.Instant

// The shapes cedar-chat's `/api` returns (see chat/README.md, "API"), as iOS/Cedar/Models/API.swift
// has them. Field names are the server's, in snake_case; dates are RFC 3339.

/** `GET /api/config`. */
@Serializable
data class ServerConfig(
    @SerialName("auth_required") val authRequired: Boolean = false,
    @SerialName("admin_enabled") val adminEnabled: Boolean = false,
    /** The server has `/api/memory`; an older one leaves it out, so Settings hides Memory. */
    val memory: Boolean = false,
    /** The server has `/api/records`; an older one leaves it out, so Settings hides Records. */
    val records: Boolean = false,
    /** The server can send a conversation to a training corpus (`CHAT_CONTRIBUTIONS_URL`). */
    val contributions: Boolean = false,
)

// -- contributing a conversation (chat/README.md, "Contributing a conversation") --------------

/**
 * `GET /api/conversations/{id}/contribution`: the conversation as it would be contributed, and
 * why it can't be, if it can't. `bundle` is the document itself (`contrib-schema`), kept as JSON:
 * what Send posts back is exactly this object, outcome added.
 */
@Serializable
data class ContributionPreview(
    val bundle: JsonObject = JsonObject(emptyMap()),
    /** `scheduled`, `active`, `no_turns`, `private_tools`, `run_gone`, `secrets`, `not_consistent`, `model_not_allowed`. */
    val blocked: List<String> = emptyList(),
    val secrets: List<String> = emptyList(),
    val problems: List<String> = emptyList(),
    @SerialName("can_send") val canSend: Boolean = false,
    val accepted: Boolean = false,
    @SerialName("private_tools") val privateTools: List<String> = emptyList(),
    /** The chat's id of each turn in the bundle, in order, for the rating buttons. */
    @SerialName("turn_ids") val turnIds: List<String> = emptyList(),
) {
    /** One turn of the bundle, as the review shows it. */
    data class PreviewTurn(
        val seq: Int,
        val prompt: String,
        val status: String,
        val day: String,
        val rating: Int?,
        /** The agent's own text, in order. */
        val agentText: List<String>,
        /** The tools it called, in order, with their inputs pretty-printed. */
        val toolCalls: List<Pair<String, String>>,
    )

    val modelName: String get() = bundle["model"]["name"].str.orEmpty()

    /** The stand-ins the redaction wrote, by kind: values a reader may take for real and shouldn't. */
    val standIns: Map<String, List<String>>
        get() = (bundle["redactions"] as? JsonObject)?.mapValues { (_, v) -> v.arr.orEmpty().mapNotNull { it.str } }.orEmpty()

    val standInCount: Int get() = standIns.values.sumOf { it.size }

    /** What was left out, as `{attachments, memory, fetched_bodies, reasoning}`. */
    val dropped: JsonElement? get() = bundle["dropped"]

    fun turns(): List<PreviewTurn> = bundle["turns"].arr.orEmpty().map { t ->
        val messages = t["messages"].arr.orEmpty()
        val agentText = messages.drop(1).filter { it["role"].str == "assistant" }
            .flatMap { m -> m["content"].arr.orEmpty() }.filter { it["type"].str == "text" }.mapNotNull { it["text"].str }
        val toolCalls = messages.flatMap { m -> m["content"].arr.orEmpty() }.filter { it["type"].str == "tool_use" }
            .map { (it["name"].str ?: "?") to it["input"].pretty() }
        PreviewTurn(
            seq = t["seq"].int ?: 0,
            prompt = t["prompt"].str.orEmpty(),
            status = t["status"].str.orEmpty(),
            day = t["day"].str.orEmpty(),
            rating = t["rating"].int,
            agentText = agentText,
            toolCalls = toolCalls,
        )
    }

    /** The bundle with the person's outcome and a turn's rating applied, as Send posts it. */
    fun bundleWith(outcome: String?, notes: String?, ratings: Map<Int, Int?>): JsonObject {
        val map = bundle.toMutableMap()
        if (outcome != null) {
            map["outcome"] = buildJsonObject {
                put("result", outcome)
                notes?.trim()?.takeIf { it.isNotEmpty() }?.let { put("notes", it) }
            }
        } else {
            map.remove("outcome")
        }
        if (ratings.isNotEmpty()) {
            map["turns"] = JsonArray(bundle["turns"].arr.orEmpty().map { t ->
                val seq = t["seq"].int ?: 0
                if (seq !in ratings) t else {
                    val tm = (t as JsonObject).toMutableMap()
                    val r = ratings[seq]
                    if (r == null) tm.remove("rating") else tm["rating"] = JsonPrimitive(r)
                    JsonObject(tm)
                }
            })
        }
        return JsonObject(map)
    }
}

/** A contribution's receipt (`GET /api/contributions`). */
@Serializable
data class Contribution(
    val id: String,
    @SerialName("conversation_id") val conversationId: String? = null,
    val title: String? = null,
    val license: String = "",
    @SerialName("submitted_at") val submittedAt: Instant = Instant.EPOCH,
    @SerialName("withdrawn_at") val withdrawnAt: Instant? = null,
    /** What the corpus last said: `quarantined`, `accepted`, `held`, `rejected`, `released`, `unknown`. */
    val status: String? = null,
    @SerialName("released_in") val releasedIn: List<String> = emptyList(),
    @SerialName("checked_at") val checkedAt: Instant? = null,
)

@Serializable
data class ContributionList(val contributions: List<Contribution> = emptyList())

/** `GET /api/settings/contributions`: the one-time choices, and whether this server can send at all. */
@Serializable
data class ContributionSettings(
    val license: String = "cc0",
    val pseudonym: String? = null,
    @SerialName("include_reasoning") val includeReasoning: Boolean = false,
    @SerialName("include_fetched") val includeFetched: Boolean = false,
    /** Set once the person read the page and turned contributing on. */
    @SerialName("accepted_at") val acceptedAt: Instant? = null,
    val available: Boolean = false,
    val host: String? = null,
    val licenses: List<String> = emptyList(),
) {
    val enabled: Boolean get() = acceptedAt != null
}

/**
 * One thing cedar remembers about the user across conversations (`GET /api/memory`): a fact,
 * preference, instruction or temporary fact they stated, an episode (what a conversation was
 * about) or a procedure (how a task went well). See "Memory" in the top-level README.
 */
@Serializable
data class MemoryItem(
    val id: String,
    val text: String,
    /** `fact`, `preference`, `instruction`, `temporary`, `episode` or `procedure`; `fact` on a cedar from before kinds. */
    val kind: String = "fact",
    /** The user's own words it was taken from, when a conversation stated it. */
    val support: String? = null,
    /** Always in cedar's prompt; never archived. */
    val pinned: Boolean = false,
    /** `user`, `agent`, `consolidation` or `api`. */
    val source: String? = null,
    @SerialName("recall_count") val recallCount: Int = 0,
    /** The conversation it was saved in, while that still exists on this server. */
    @SerialName("conversation_id") val conversationId: String? = null,
    /** That conversation's title. */
    val title: String? = null,
    @SerialName("created_at") val createdAt: Instant = Instant.EPOCH,
    @SerialName("updated_at") val updatedAt: Instant = Instant.EPOCH,
    @SerialName("valid_from") val validFrom: Instant? = null,
    @SerialName("valid_until") val validUntil: Instant? = null,
    /** Out of cedar's prompt, kept until deleted; `supersededBy` names the newer version. */
    @SerialName("archived_at") val archivedAt: Instant? = null,
    @SerialName("superseded_by") val supersededBy: String? = null,
) {
    val isActive: Boolean get() = archivedAt == null && supersededBy == null
    val isAboutUser: Boolean get() = kind != "episode" && kind != "procedure"
}

/**
 * The short summary of the user cedar reads before every reply, written by its background pass
 * unless the user wrote it (`userEdited`; then it stays as written).
 */
@Serializable
data class MemoryProfile(
    val text: String,
    @SerialName("user_edited") val userEdited: Boolean = false,
    @SerialName("updated_at") val updatedAt: Instant = Instant.EPOCH,
)

/** The answer to `GET` and `PUT /api/memory/profile`. */
@Serializable
data class MemoryProfileResponse(val profile: MemoryProfile? = null)

/**
 * Which memories Settings → Memory lists: the rows about the user, past conversations, ways of
 * working, or the archive (superseded, expired and forgotten rows). `kind` is the query, null for
 * the server's default (the rows about the user).
 */
enum class MemoryFilter(val kind: String?, val archived: Boolean) {
    ABOUT(null, false), CONVERSATIONS("episode", false), PROCEDURES("procedure", false), ARCHIVE(null, true);

    companion object {
        /** Every kind of active memory, for counting them all whatever the picker shows. */
        const val ALL_KINDS = "fact,preference,instruction,temporary,episode,procedure"
    }
}

@Serializable
data class MemoryLimits(
    @SerialName("max_items") val maxItems: Int = 0,
    @SerialName("max_chars") val maxChars: Int = 0,
    @SerialName("max_profile_chars") val maxProfileChars: Int = 0,
)

/** What cedar remembers, oldest first, with the profile; `supported` is false on a cedar from before memory. */
@Serializable
data class MemoryList(
    val supported: Boolean = false,
    val items: List<MemoryItem> = emptyList(),
    val profile: MemoryProfile? = null,
    val limits: MemoryLimits? = null,
)

/**
 * One set of records cedar keeps for the user across conversations (`GET /api/records`): a
 * running log, weekly measurements (see the top-level README, "Records").
 */
@Serializable
data class RecordSet(
    val id: String,
    val name: String,
    val description: String = "",
    /** The fields that identify a row; empty when every save appends. */
    @SerialName("key_fields") val keyFields: List<String> = emptyList(),
    @SerialName("health_data") val healthData: Boolean = false,
    val count: Int = 0,
    val columns: List<String> = emptyList(),
    @SerialName("created_at") val createdAt: Instant = Instant.EPOCH,
    @SerialName("updated_at") val updatedAt: Instant = Instant.EPOCH,
)

@Serializable
data class RecordsLimits(
    @SerialName("max_sets") val maxSets: Int = 0,
    @SerialName("max_items") val maxItems: Int = 0,
)

/** The sets cedar keeps, oldest first; `supported` is false on a cedar from before records. */
@Serializable
data class RecordsList(
    val supported: Boolean = false,
    val sets: List<RecordSet> = emptyList(),
    val limits: RecordsLimits? = null,
)

/**
 * One row of a set (`GET /api/records/{name}`), with the conversation whose run saved it last,
 * while that still exists on this server.
 */
@Serializable
data class RecordRow(
    val id: String,
    val key: String = "",
    val data: JsonObject = JsonObject(emptyMap()),
    @SerialName("run_id") val runId: String? = null,
    @SerialName("conversation_id") val conversationId: String? = null,
    val title: String? = null,
    @SerialName("created_at") val createdAt: Instant = Instant.EPOCH,
    @SerialName("updated_at") val updatedAt: Instant = Instant.EPOCH,
)

/** A set with all its rows, oldest first. */
@Serializable
data class RecordSetDetail(
    val set: RecordSet,
    val records: List<RecordRow> = emptyList(),
)

/** One of the models cedar has registered (`GET /api/models`). */
@Serializable
data class ModelInfo(
    val id: String,
    @SerialName("label") private val rawLabel: String? = null,
    @SerialName("model") private val rawModel: String? = null,
    @SerialName("reasoning_levels") val reasoningLevels: List<String> = emptyList(),
    @SerialName("default_reasoning") val defaultReasoning: String? = null,
    val multimodal: Boolean = false,
    @SerialName("is_default") val isDefault: Boolean = false,
) {
    val model: String get() = rawModel ?: id
    val label: String get() = rawLabel ?: model
}

@Serializable
data class ModelList(val models: List<ModelInfo> = emptyList())

/**
 * A conversation as cedar-chat keeps it: its title, the model it last used, and whether a finished
 * response is waiting to be read or answered.
 */
@Serializable
data class Conversation(
    val id: String,
    val title: String? = null,
    val model: String? = null,
    val reasoning: String? = null,
    /** The agent whose run this is (the server's name for it is from when agents were schedules). */
    @SerialName("schedule_id") val scheduleId: String? = null,
    val tools: List<String>? = null,
    val unread: Boolean = false,
    @SerialName("needs_answer") val needsAnswer: Boolean = false,
    /** When it was starred, on any device; `null` when it isn't. */
    @SerialName("starred_at") val starredAt: Instant? = null,
    /** The title of the agent a run belongs to; only the Starred list and a search send it. */
    @SerialName("schedule_title") val scheduleTitle: String? = null,
    /** The passage of a message a search matched; only search results carry it. */
    val snippet: String? = null,
    @SerialName("created_at") val createdAt: Instant,
    @SerialName("updated_at") val updatedAt: Instant,
) {
    /** The title cedar-chat gave it (the first message), or a placeholder. */
    val displayTitle: String get() = titleOr("New chat")

    /** The title cedar-chat gave it, or `placeholder` (the app's, in the user's language). */
    fun titleOr(placeholder: String): String = title?.trim().orEmpty().ifEmpty { placeholder }
    val isStarred: Boolean get() = starredAt != null
}

/** Where the words of `query` are in `text`, whatever their case, in the order they come: what a search result marks. */
fun searchRanges(text: String, query: String): List<IntRange> =
    query.split(Regex("\\s+")).filter { it.isNotEmpty() }.distinct()
        .flatMap { word ->
            generateSequence(text.indexOf(word, ignoreCase = true).takeIf { it >= 0 }) { at ->
                text.indexOf(word, at + word.length, ignoreCase = true).takeIf { it >= 0 }
            }.map { it until it + word.length }.toList()
        }
        .sortedBy { it.first }

@Serializable
data class ConversationList(val conversations: List<Conversation> = emptyList())

/**
 * `GET /api/conversations/{id}`; `agent` is there when the conversation is an agent's run, and
 * `followups` holds what was decided about the follow-ups its turns suggested.
 */
@Serializable
data class ConversationDetail(
    val conversation: Conversation,
    val turns: List<Turn> = emptyList(),
    val agent: AgentRef? = null,
    val followups: List<Followup> = emptyList(),
)

@Serializable
enum class TurnStatus {
    @SerialName("queued") QUEUED,
    @SerialName("running") RUNNING,
    @SerialName("completed") COMPLETED,
    @SerialName("failed") FAILED,
    @SerialName("cancelled") CANCELLED;

    val isTerminal: Boolean get() = this != QUEUED && this != RUNNING
}

/** A file sent with a message, as the server reports it back. */
@Serializable
data class AttachmentInfo(
    val name: String,
    @SerialName("media_type") val mediaType: String,
    val size: Int = 0,
)

/** One message and the cedar run that answers it. */
@Serializable
data class Turn(
    val id: String,
    @SerialName("conversation_id") val conversationId: String,
    val seq: Int = 0,
    @SerialName("run_id") val runId: String,
    val prompt: String = "",
    val status: TurnStatus,
    @SerialName("final_text") val finalText: String? = null,
    val error: String? = null,
    @SerialName("stop_reason") val stopReason: String? = null,
    @SerialName("finished_at") val finishedAt: Instant? = null,
    val seen: Boolean = true,
    val attachments: List<AttachmentInfo> = emptyList(),
    @SerialName("created_at") val createdAt: Instant = Instant.EPOCH,
) {
    val isActive: Boolean get() = !status.isTerminal

    /** The agent stopped to ask something (`ask_user`); the next message answers it. */
    val needsAnswer: Boolean get() = status == TurnStatus.COMPLETED && stopReason == "ask_user"
}

/** A finished response nobody has looked at (`GET /api/inbox`). */
@Serializable
data class InboxItem(
    @SerialName("turn_id") val turnId: String,
    @SerialName("conversation_id") val conversationId: String,
    val title: String? = null,
    @SerialName("schedule_id") val scheduleId: String? = null,
    val status: String = "",
    @SerialName("stop_reason") val stopReason: String? = null,
    @SerialName("needs_answer") val needsAnswer: Boolean = false,
    @SerialName("finished_at") val finishedAt: Instant? = null,
)

@Serializable
data class Inbox(val items: List<InboxItem> = emptyList(), val unread: Int = 0)

/** A `turn.finished` notice from `GET /api/events`: a response ended somewhere. */
@Serializable
data class Notice(
    @SerialName("turn_id") val turnId: String,
    @SerialName("conversation_id") val conversationId: String,
    @SerialName("run_id") val runId: String? = null,
    val status: String = "",
    @SerialName("stop_reason") val stopReason: String? = null,
    val error: String? = null,
    @SerialName("needs_answer") val needsAnswer: Boolean = false,
    val title: String? = null,
    @SerialName("schedule_id") val scheduleId: String? = null,
)

/** One entry of a run's event log, as cedar-chat relays it from cedar over Server-Sent Events. */
@Serializable
data class RunEvent(val seq: Int, val type: String, val data: JsonElement = JsonNull) {
    val isTerminal: Boolean get() = type in TERMINAL_TYPES

    companion object {
        val TERMINAL_TYPES = setOf("run.completed", "run.failed", "run.cancelled")
    }
}

/** A file under the conversation's `/workspace` (`GET /api/conversations/{id}/files`). */
@Serializable
data class WorkspaceFile(
    val path: String,
    val size: Long = 0,
    @SerialName("modified_at") val modifiedAt: Instant? = null,
)

@Serializable
data class WorkspaceFiles(
    @SerialName("workspace_id") val workspaceId: String? = null,
    val files: List<WorkspaceFile> = emptyList(),
    val truncated: Boolean = false,
)

/** A phone registered for Android push (`POST /api/notifications/android/devices`). */
@Serializable
data class AndroidDevice(
    val id: String,
    val transport: String,
    val name: String? = null,
)

@Serializable
data class AndroidDeviceResponse(val device: AndroidDevice? = null)
