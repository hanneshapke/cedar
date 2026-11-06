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
import java.util.TimeZone

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

/** One fact cedar remembers about the user across conversations (`GET /api/memory`). */
@Serializable
data class MemoryItem(
    val id: String,
    val text: String,
    /** The conversation it was saved in, while that still exists on this server. */
    @SerialName("conversation_id") val conversationId: String? = null,
    /** That conversation's title. */
    val title: String? = null,
    @SerialName("created_at") val createdAt: Instant = Instant.EPOCH,
    @SerialName("updated_at") val updatedAt: Instant = Instant.EPOCH,
)

@Serializable
data class MemoryLimits(
    @SerialName("max_items") val maxItems: Int = 0,
    @SerialName("max_chars") val maxChars: Int = 0,
)

/** What cedar remembers, oldest first; `supported` is false on a cedar from before memory. */
@Serializable
data class MemoryList(
    val supported: Boolean = false,
    val items: List<MemoryItem> = emptyList(),
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
    @SerialName("schedule_id") val scheduleId: String? = null,
    val tools: List<String>? = null,
    val unread: Boolean = false,
    @SerialName("needs_answer") val needsAnswer: Boolean = false,
    /** When it was starred, on any device; `null` when it isn't. */
    @SerialName("starred_at") val starredAt: Instant? = null,
    /** The schedule a scheduled run belongs to; only the Starred list sends it. */
    @SerialName("schedule_title") val scheduleTitle: String? = null,
    @SerialName("created_at") val createdAt: Instant,
    @SerialName("updated_at") val updatedAt: Instant,
) {
    /** The title cedar-chat gave it (the first message), or a placeholder. */
    val displayTitle: String get() = titleOr("New chat")

    /** The title cedar-chat gave it, or `placeholder` (the app's, in the user's language). */
    fun titleOr(placeholder: String): String = title?.trim().orEmpty().ifEmpty { placeholder }
    val isStarred: Boolean get() = starredAt != null
}

@Serializable
data class ConversationList(val conversations: List<Conversation> = emptyList())

@Serializable
data class ConversationDetail(val conversation: Conversation, val turns: List<Turn> = emptyList())

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

/**
 * A scheduled agent (`GET /api/schedules`): a prompt the server starts as a fresh run whenever it is
 * due, each run in a conversation of its own that the conversation list leaves out.
 */
@Serializable
data class Schedule(
    val id: String,
    val title: String,
    val prompt: String = "",
    val model: String? = null,
    val reasoning: String? = null,
    /** `daily`, `weekdays` or `weekly`. */
    val frequency: String,
    /** 0–6, Monday–Sunday; weekly only. */
    val weekday: Int? = null,
    /** `HH:MM` in `timezone`. */
    @SerialName("time_of_day") val timeOfDay: String,
    val timezone: String,
    val enabled: Boolean = true,
    @SerialName("next_run_at") val nextRunAt: Instant,
    @SerialName("last_run_at") val lastRunAt: Instant? = null,
    /** Only in the list; a schedule as `PATCH` returns it has none. */
    @SerialName("last_run") val lastRun: ScheduleRun? = null,
) {
    /** "Weekdays at 07:30", with the schedule's time zone when it isn't this device's. */
    fun cadence(deviceZone: String = TimeZone.getDefault().id): String {
        val names = listOf("Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays")
        val whenText = when (frequency) {
            "daily" -> "Daily"
            "weekdays" -> "Weekdays"
            "weekly" -> weekday?.let { names.getOrNull(it) } ?: "Weekly"
            else -> frequency.replaceFirstChar { it.uppercase() }
        }
        val zone = zoneLabel(deviceZone)?.let { " ($it)" } ?: ""
        return "$whenText at $timeOfDay$zone"
    }

    /** The schedule's time zone, when it isn't this device's. */
    fun zoneLabel(deviceZone: String = TimeZone.getDefault().id): String? = timezone.takeIf { it != deviceZone }
}

/**
 * One time a schedule fired, with the turn that carries its run, or the `error` that kept it from
 * starting.
 */
@Serializable
data class ScheduleRun(
    val id: String,
    @SerialName("schedule_id") val scheduleId: String,
    @SerialName("conversation_id") val conversationId: String? = null,
    @SerialName("turn_id") val turnId: String? = null,
    @SerialName("due_at") val dueAt: Instant,
    @SerialName("started_at") val startedAt: Instant,
    /** Started with Run now rather than on schedule. */
    val manual: Boolean = false,
    val error: String? = null,
    val turn: Turn? = null,
) {
    enum class Outcome { QUEUED, RUNNING, COMPLETED, NEEDS_ANSWER, FAILED, CANCELLED, NOT_STARTED }

    val outcome: Outcome
        get() {
            val turn = turn ?: return Outcome.NOT_STARTED
            return when (turn.status) {
                TurnStatus.QUEUED -> Outcome.QUEUED
                TurnStatus.RUNNING -> Outcome.RUNNING
                TurnStatus.COMPLETED -> if (turn.needsAnswer) Outcome.NEEDS_ANSWER else Outcome.COMPLETED
                TurnStatus.FAILED -> Outcome.FAILED
                TurnStatus.CANCELLED -> Outcome.CANCELLED
            }
        }

    val isActive: Boolean get() = turn?.isActive ?: false

    /** A finished response in it nobody has looked at. */
    val unread: Boolean get() = turn?.let { !it.isActive && !it.seen } ?: false
}

@Serializable
data class ScheduleList(val schedules: List<Schedule> = emptyList())

/** `GET /api/schedules/{id}`: the schedule and its runs, newest first. */
@Serializable
data class ScheduleDetail(val schedule: Schedule, val runs: List<ScheduleRun> = emptyList())

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
