@file:UseSerializers(InstantSerializer::class)

package dev.cedar.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.time.DayOfWeek
import java.time.Instant
import java.time.LocalDateTime
import java.time.format.DateTimeFormatter

// Today (chat/src/api/timeline.rs, outcome.rs, asks.rs): one day as a river of what Cedar did
// (earlier), is doing or waits on the person for (now), and will do (ahead); what one ask came
// to (its outcome); and the one way to ask, with a When. The web's /today/ page shows the same.

/**
 * What a turn's run waits on from the person while it goes: its plan to be agreed to
 * (`kind` is `plan`), or a risky call to be approved (`approval`, keyed `approval:<id>`).
 */
@Serializable
data class TurnWait(
    val kind: String = "",
    val key: String = "",
    /** An approval's reason, as cedar gave it. */
    val reason: String? = null,
    val since: Instant? = null,
) {
    val isPlan: Boolean get() = kind == PLAN
    val isApproval: Boolean get() = kind == APPROVAL

    /** The approval's id, for `POST /api/turns/{id}/approvals/{approval_id}`; `null` for a plan. */
    val approvalId: String?
        get() = key.removePrefix("approval:").takeIf { isApproval && key.startsWith("approval:") && it.isNotEmpty() }

    companion object {
        const val PLAN = "plan"
        const val APPROVAL = "approval"
    }
}

/** Where a turn stands, in the river's words. A state from a newer server is [OTHER]. */
enum class TurnState(val id: String) {
    /** On the person's go-ahead or OK. */
    WAITING("waiting"),

    /** It ended with a question nobody answered yet. */
    ASKING("asking"),
    WORKING("working"),
    DONE("done"),
    FAILED("failed"),
    STOPPED("stopped"),
    OTHER("");

    /** The person has something to do about it. */
    val needsYou: Boolean get() = this == WAITING || this == ASKING

    companion object {
        fun of(id: String): TurnState = entries.firstOrNull { it.id == id && it != OTHER } ?: OTHER
    }
}

/** A turn as the river shows it: what was asked, how it went, what it belongs to, what it waits on. */
@Serializable
data class TimelineTurn(
    @SerialName("turn_id") val turnId: String,
    @SerialName("conversation_id") val conversationId: String,
    val title: String? = null,
    val prompt: String = "",
    /** See [TurnState]. */
    @SerialName("state") val stateId: String = "",
    /** The response's outcome in one line; `null` while it has none. */
    val summary: String? = null,
    val error: String? = null,
    @SerialName("created_at") val createdAt: Instant = Instant.EPOCH,
    @SerialName("finished_at") val finishedAt: Instant? = null,
    val seen: Boolean = true,
    @SerialName("needs_answer") val needsAnswer: Boolean = false,
    /** The agent whose run this is. */
    val agent: AgentRef? = null,
    val waits: List<TurnWait> = emptyList(),
) {
    val state: TurnState get() = TurnState.of(stateId)

    /** What names the row: the conversation's title, else what was asked. */
    val label: String get() = title?.trim().orEmpty().ifEmpty { prompt.trim() }

    /** The line under the name: why it failed, else what it came to. */
    val line: String? get() = (if (state == TurnState.FAILED) error else summary)?.trim()?.takeIf { it.isNotEmpty() }
}

/**
 * `GET /api/timeline`: one day. [now], [outbox], [ahead] and [watching] are today's only; what
 * needs the person stays under now whatever day it was asked. The agents are as the river lists
 * them: title, trigger, whether they are on, and when they run next.
 */
@Serializable
data class Timeline(
    /** `YYYY-MM-DD`, in [timezone]. */
    val day: String = "",
    val timezone: String = "",
    val today: Boolean = true,
    /** Turns that wait or ask, and the calls held in the outbox. */
    @SerialName("needs_you") val needsYou: Int = 0,
    /** The day's turns that are over, oldest first. */
    val earlier: List<TimelineTurn> = emptyList(),
    /** Still going, waiting on the person, or asking; oldest first. */
    val now: List<TimelineTurn> = emptyList(),
    /** What unattended runs held for the person's OK, oldest first. */
    val outbox: List<OutboxItem> = emptyList(),
    /** Agents on a clock, by when they run next. */
    val ahead: List<Agent> = emptyList(),
    /** Agents that something outside starts: they have no next run. */
    val watching: List<Agent> = emptyList(),
    /** Every agent, for the list of standing asks. */
    val standing: List<Agent> = emptyList(),
) {
    val isEmpty: Boolean get() = earlier.isEmpty() && now.isEmpty() && outbox.isEmpty() && ahead.isEmpty() && watching.isEmpty()
}

// -- one ask's outcome ---------------------------------------------------------------------------

/** One tool call of the run, in order. */
@Serializable
data class OutcomeStep(val id: String = "", val name: String = "", @SerialName("is_error") val isError: Boolean = false)

/** Something the run read: a page (`url`) or a document of the workspace (`path`). */
@Serializable
data class OutcomeSource(val url: String? = null, val path: String? = null, val title: String? = null) {
    val label: String get() = title?.takeIf { it.isNotBlank() } ?: url ?: path.orEmpty()

    /** Only http(s) addresses are opened. */
    val link: String? get() = url?.takeIf { it.startsWith("https://", ignoreCase = true) || it.startsWith("http://", ignoreCase = true) }
}

/** A `suggest_follow_up` call's result, as the tool wrote it ([FollowUpSuggestion.parse] reads it). */
@Serializable
data class OutcomeFollowup(@SerialName("call_id") val callId: String = "", val output: String = "") {
    val suggestion: FollowUpSuggestion? get() = FollowUpSuggestion.parse(output)
}

/** Something the run remembered about the user. */
@Serializable
data class OutcomeMemory(val id: String, val text: String = "", val kind: String = "fact")

/** A record row the run saved last. */
@Serializable
data class OutcomeRecord(val set: String = "", val key: String? = null, val data: JsonElement? = null) {
    val label: String get() = key?.takeIf { it.isNotEmpty() }?.let { "$set: $it" } ?: set
}

/** The conversation an outcome's turn is in. */
@Serializable
data class OutcomeConversation(val id: String, val title: String? = null, @SerialName("private") val isPrivate: Boolean = false)

/**
 * `GET /api/turns/{id}/outcome`: what one ask came to. The answer is the turn's `finalText`;
 * the rest is what it left behind, and how it got there. A part cedar could not give is empty.
 */
@Serializable
data class Outcome(
    val turn: Turn,
    val conversation: OutcomeConversation,
    val agent: AgentRef? = null,
    @SerialName("state") val stateId: String = "",
    val summary: String? = null,
    /** How many messages the conversation has after this one. */
    @SerialName("later_turns") val laterTurns: Int = 0,
    val steps: List<OutcomeStep> = emptyList(),
    val sources: List<OutcomeSource> = emptyList(),
    val followups: List<OutcomeFollowup> = emptyList(),
    @SerialName("followup_decisions") val followupDecisions: List<Followup> = emptyList(),
    val memories: List<OutcomeMemory> = emptyList(),
    val records: List<OutcomeRecord> = emptyList(),
    /** The workspace files that changed since the turn was sent. */
    val files: List<WorkspaceFile> = emptyList(),
    /** Every call the run held for approval, whatever became of it. */
    val held: List<OutboxItem> = emptyList(),
) {
    val state: TurnState get() = TurnState.of(stateId)

    /** The outcome in a line: why it failed, else the answer's gist, else what was asked. */
    val headline: String
        get() {
            val said = if (state == TurnState.FAILED) turn.error else summary
            return said?.trim()?.takeIf { it.isNotEmpty() } ?: conversation.title?.trim()?.takeIf { it.isNotEmpty() } ?: turn.prompt.trim()
        }

    val failedSteps: Int get() = steps.count { it.isError }

    /** What was decided about a suggested follow-up; `null` while it is open. */
    fun decision(callId: String): Followup? = followupDecisions.firstOrNull { it.callId == callId }

    /** The held calls, the ones still to decide first. */
    val heldOpenFirst: List<OutboxItem> get() = held.sortedBy { if (it.isOpen) 0 else 1 }
}

// -- asking, with a When -------------------------------------------------------------------------

/** When an ask runs: the server's `when`. */
enum class AskWhen(val id: String) {
    /** A new conversation, at once. */
    NOW("now"),

    /** One run at a time the person picks. */
    LATER("later"),

    /** On a clock: daily, on weekdays or weekly. */
    EVERY("every"),

    /** When something happens: mail, the calendar, the person's own device. */
    WATCH("watch"),
}

/** How often an [AskWhen.EVERY] ask runs; the server's `frequency`. */
enum class AskFrequency(val id: String) { DAILY("daily"), WEEKDAYS("weekdays"), WEEKLY("weekly") }

/** What an [AskWhen.WATCH] ask waits for. */
enum class AskWatch { EMAIL, CALENDAR, DEVICE }

/** Why an ask can't be sent yet; the app words each in the user's language. */
enum class AskProblem { NO_TEXT, NO_TIME, TIME_PASSED, NO_QUERY, BAD_MINUTES }

/**
 * `POST /api/asks`, checked: one sentence and a When. With [AskWhen.NOW] there is no trigger and
 * the ask may be off the record; any other When makes an agent of it, and [trigger] says when.
 */
data class AskBody(
    val content: String,
    val whenKind: AskWhen = AskWhen.NOW,
    val trigger: Trigger? = null,
    val private: Boolean = false,
    /** The user's locale as a BCP 47 tag, as a message sends it. */
    val locale: String? = null,
    /** The device's IANA time zone. */
    val timezone: String? = null,
) {
    /** Exactly what the server takes: it refuses fields it doesn't know, and a trigger with now. */
    fun toJson(): JsonObject = buildJsonObject {
        put("content", content)
        if (whenKind == AskWhen.NOW) {
            if (private) put("private", true)
            locale?.let { put("locale", it) }
        } else {
            put("when", whenKind.id)
            trigger?.let { put("trigger", it.toJson()) }
        }
        timezone?.let { put("timezone", it) }
    }
}

/** The answer to an ask: the conversation and its first turn (now), or the agent it became. */
@Serializable
data class AskResult(
    @SerialName("when") val whenId: String = "now",
    val conversation: Conversation? = null,
    val turn: Turn? = null,
    val agent: Agent? = null,
)

/**
 * The ask being written: its When and that When's details, as the form holds them. [body] turns
 * it into what is sent; [problem] says what is still missing.
 */
data class AskDraft(
    val whenKind: AskWhen = AskWhen.NOW,
    /** [AskWhen.LATER]: the local date and time of the one run. */
    val laterAt: LocalDateTime? = null,
    val frequency: AskFrequency = AskFrequency.DAILY,
    /** [AskFrequency.WEEKLY]: 0 is Monday, as the server counts. */
    val weekday: Int = 0,
    /** [AskWhen.EVERY]: `HH:MM`. */
    val time: String = "07:00",
    val watch: AskWatch = AskWatch.EMAIL,
    /** [AskWatch.EMAIL]: Gmail's search syntax. */
    val query: String = "",
    /** [AskWatch.CALENDAR]: how long before an event starts. */
    val minutesBefore: Int = 30,
    /** Off the record; only an ask that runs now can be. */
    val private: Boolean = false,
) {
    /** Off the record only counts with now: nothing of such an ask is kept for later. */
    val isPrivate: Boolean get() = private && whenKind == AskWhen.NOW

    /** What starts the agent's runs; `null` for now, and while [problem] says something is missing. */
    fun trigger(timezone: String): Trigger? = when (whenKind) {
        AskWhen.NOW -> null
        AskWhen.LATER -> laterAt?.let { Trigger.Once(it.withSecond(0).withNano(0).format(AT), timezone) }
        AskWhen.EVERY -> Trigger.Time(frequency.id, weekday.takeIf { frequency == AskFrequency.WEEKLY }, time, timezone)
        AskWhen.WATCH -> when (watch) {
            AskWatch.EMAIL -> query.trim().takeIf { it.isNotEmpty() }?.let { Trigger.Email(it) }
            AskWatch.CALENDAR -> minutesBefore.takeIf { it in MINUTES }?.let { Trigger.Calendar(minutesBefore = it) }
            AskWatch.DEVICE -> Trigger.Device
        }
    }

    /** What keeps `content` from being sent now, if anything. `now` is the device's local time. */
    fun problem(content: String, now: LocalDateTime = LocalDateTime.now()): AskProblem? = when {
        content.isBlank() -> AskProblem.NO_TEXT
        whenKind == AskWhen.LATER && laterAt == null -> AskProblem.NO_TIME
        whenKind == AskWhen.LATER && laterAt?.isAfter(now) != true -> AskProblem.TIME_PASSED
        whenKind == AskWhen.WATCH && watch == AskWatch.EMAIL && query.isBlank() -> AskProblem.NO_QUERY
        whenKind == AskWhen.WATCH && watch == AskWatch.CALENDAR && minutesBefore !in MINUTES -> AskProblem.BAD_MINUTES
        else -> null
    }

    /** The ask as it is sent. Call once [problem] is `null`. */
    fun body(content: String, timezone: String, locale: String? = null): AskBody =
        AskBody(content.trim(), whenKind, trigger(timezone), isPrivate, locale, timezone)

    companion object {
        /** The server's `2026-12-03T09:00`. */
        private val AT: DateTimeFormatter = DateTimeFormatter.ofPattern("yyyy-MM-dd'T'HH:mm")

        /** How long before an event a calendar ask may run. */
        val MINUTES = 1..1440

        /** A draft whose later is the next full hour two hours from `now`, as the web's is. */
        fun starting(now: LocalDateTime = LocalDateTime.now()): AskDraft =
            AskDraft(laterAt = now.plusHours(2).withMinute(0).withSecond(0).withNano(0))

        /** The server's weekday (0 is Monday) for a day of the week. */
        fun weekday(day: DayOfWeek): Int = day.value - 1
    }
}
