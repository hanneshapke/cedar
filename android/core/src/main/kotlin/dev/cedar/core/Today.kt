@file:UseSerializers(InstantSerializer::class)

package dev.cedar.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
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
    /** A plan's steps, as cedar proposed them; `null` from a server that doesn't send them. */
    val steps: List<WaitStep>? = null,
    val since: Instant? = null,
) {
    /** The plan as the chat's go-ahead takes it; empty when the steps aren't known here. */
    val planSteps: List<PlanStep> get() = steps.orEmpty().filter { it.id.isNotEmpty() && it.title.isNotBlank() }.map { PlanStep(it.id, it.title) }

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

/** One step of a plan that waits for its go-ahead. */
@Serializable
data class WaitStep(val id: String = "", val title: String = "")

/** Where a response still going is: how many tool calls its run has started, and the latest one's name. */
@Serializable
data class TurnProgress(val steps: Int = 0, val doing: String? = null)

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
    /** Only while it is going, and once its run has called a tool. */
    val progress: TurnProgress? = null,
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

    /** The row's fields as `name` and value, the first [MAX_FIELDS] that have one. */
    val fields: List<Pair<String, String>>
        get() = (data as? JsonObject).orEmpty().mapNotNull { (name, value) ->
            val text = when (value) {
                is JsonNull -> ""
                is JsonPrimitive -> value.content
                else -> value.toString()
            }.trim()
            if (text.isEmpty()) null else name to (if (text.length > MAX_VALUE) text.take(MAX_VALUE - 1) + "…" else text)
        }.take(MAX_FIELDS)

    companion object {
        const val MAX_FIELDS = 8
        const val MAX_VALUE = 120
    }
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

    /** The answer without the paragraph the headline already says (see [answerBelowHeadline]). */
    val answer: String? get() = turn.finalText?.let { answerBelowHeadline(it, if (state == TurnState.FAILED) null else summary) }?.takeIf { it.isNotBlank() }

    /** What was decided about a suggested follow-up; `null` while it is open. */
    fun decision(callId: String): Followup? = followupDecisions.firstOrNull { it.callId == callId }

    /** The held calls, the ones still to decide first. */
    val heldOpenFirst: List<OutboxItem> get() = held.sortedBy { if (it.isOpen) 0 else 1 }
}

/**
 * `text` (markdown) without its first paragraph of prose when that paragraph is what `summary`
 * says, so an outcome doesn't say its headline twice; a heading left with nothing under it goes
 * too. The paragraph is read the way the server makes the summary (chat/src/api/timeline.rs):
 * markdown marks and `[n]` citations out, whitespace collapsed. Anything else is `text` as it was.
 */
fun answerBelowHeadline(text: String, summary: String?): String {
    val wanted = summary?.trim().orEmpty()
    if (wanted.isEmpty()) return text
    val lines = text.replace("\r", "").split("\n")
    var inCode = false
    var start = -1
    var end = -1
    for ((i, raw) in lines.withIndex()) {
        val line = raw.trim()
        if (line.startsWith("```")) {
            if (start >= 0) break
            inCode = !inCode
            continue
        }
        if (inCode || line.startsWith("|") || line.startsWith("---")) {
            if (start >= 0) break
            continue
        }
        if (line.isEmpty() || line.startsWith("#")) {
            if (start >= 0) break
            continue
        }
        if (start < 0) start = i
        end = i
    }
    if (start < 0) return text
    val paragraph = lines.subList(start, end + 1).joinToString(" ") { it.trim().trimStart('-', '*', '>', ' ').trim() }
    if (plainProse(paragraph) != plainProse(wanted)) return text
    val kept = lines.toMutableList()
    for (i in end downTo start) kept.removeAt(i)
    // The blank line that followed it would double the one before it.
    while (start < kept.size && kept[start].isBlank() && (start == 0 || kept[start - 1].isBlank())) kept.removeAt(start)
    // The heading the paragraph stood under, when nothing but another heading (or the end) follows.
    val before = (start - 1 downTo 0).firstOrNull { kept[it].isNotBlank() }
    val after = (start until kept.size).firstOrNull { kept[it].isNotBlank() }
    if (before != null && kept[before].trim().startsWith("#") && (after == null || kept[after].trim().startsWith("#"))) {
        kept.removeAt(before)
    }
    return kept.joinToString("\n").trim('\n')
}

/** Prose with markdown's inline marks, links and `[n]` citations taken out, on one line. */
private fun plainProse(text: String): String {
    val out = StringBuilder()
    var i = 0
    while (i < text.length) {
        val c = text[i]
        when {
            c == '*' || c == '_' || c == '`' -> i++
            c == '[' -> {
                val close = text.indexOf(']', i)
                when {
                    close > i && text.getOrNull(close + 1) == '(' -> {
                        out.append(text, i + 1, close)
                        val paren = text.indexOf(')', close)
                        i = if (paren >= 0) paren + 1 else close + 1
                    }
                    close > i + 1 && text.substring(i + 1, close).all { it.isDigit() } -> {
                        while (out.isNotEmpty() && out.last() == ' ') out.deleteCharAt(out.length - 1)
                        i = close + 1
                    }
                    else -> {
                        out.append(c)
                        i++
                    }
                }
            }
            else -> {
                out.append(c)
                i++
            }
        }
    }
    return out.toString().split(Regex("\\s+")).filter { it.isNotEmpty() }.joinToString(" ")
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
    /**
     * [AskWhen.EVERY]: a trigger the server read out of the sentence that the form has no fields
     * for (monthly, every so many minutes). It is sent as it is while Every stays picked; changing
     * how often clears it (see [edited]).
     */
    val kept: Trigger? = null,
) {
    /** The draft after the person changed Every's own fields: what was read no longer stands. */
    fun edited(): AskDraft = copy(kept = null)

    /** Off the record only counts with now: nothing of such an ask is kept for later. */
    val isPrivate: Boolean get() = private && whenKind == AskWhen.NOW

    /** What starts the agent's runs; `null` for now, and while [problem] says something is missing. */
    fun trigger(timezone: String): Trigger? = when (whenKind) {
        AskWhen.NOW -> null
        AskWhen.LATER -> laterAt?.let { Trigger.Once(it.withSecond(0).withNano(0).format(AT), timezone) }
        AskWhen.EVERY -> kept ?: Trigger.Time(frequency.id, weekday.takeIf { frequency == AskFrequency.WEEKLY }, time, timezone)
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

    /**
     * This draft with the When the server read out of the sentence ([AskReading]) picked and its
     * fields filled in. A trigger the form can't show is [kept]; a reading that isn't one of the
     * Whens, or whose trigger doesn't belong to its When, leaves the draft as it is.
     */
    fun reading(read: AskReading): AskDraft {
        val trigger = read.trigger
        return when (read.whenId) {
            AskWhen.NOW.id -> copy(whenKind = AskWhen.NOW, kept = null)
            AskWhen.LATER.id -> (trigger as? Trigger.Once)?.let { once ->
                runCatching { LocalDateTime.parse(once.at) }.getOrNull()?.let { copy(whenKind = AskWhen.LATER, laterAt = it, kept = null) }
            } ?: this
            AskWhen.EVERY.id -> when (trigger) {
                is Trigger.Time -> {
                    val frequency = AskFrequency.entries.firstOrNull { it.id == trigger.frequency }
                    if (frequency == null) copy(whenKind = AskWhen.EVERY, kept = trigger)
                    else copy(whenKind = AskWhen.EVERY, frequency = frequency, weekday = trigger.weekday?.takeIf { it in 0..6 } ?: weekday, time = trigger.time, kept = null)
                }
                is Trigger.Monthly, is Trigger.Interval -> copy(whenKind = AskWhen.EVERY, kept = trigger)
                else -> this
            }
            AskWhen.WATCH.id -> when (trigger) {
                is Trigger.Email -> copy(whenKind = AskWhen.WATCH, watch = AskWatch.EMAIL, query = trigger.query, kept = null)
                is Trigger.Calendar -> copy(whenKind = AskWhen.WATCH, watch = AskWatch.CALENDAR, minutesBefore = trigger.minutesBefore ?: minutesBefore, kept = null)
                Trigger.Device -> copy(whenKind = AskWhen.WATCH, watch = AskWatch.DEVICE, kept = null)
                else -> this
            }
            else -> this
        }
    }
}

/**
 * `POST /api/asks/draft`: the When the server's model read out of a sentence, and the sentence
 * without its timing. Nothing was made: the person sends it, or changes it first.
 */
@Serializable
data class AskReading(
    @SerialName("when") val whenId: String = "now",
    val trigger: Trigger? = null,
    val content: String = "",
)

// -- searching everything ------------------------------------------------------------------------

/** A record set a search found, by its name or description. */
@Serializable
data class FoundRecordSet(val name: String = "", val description: String = "", val count: Int = 0)

/**
 * `GET /api/search`: everything of the person's that has every word: conversations (as the
 * conversations' own search finds them), agents, what Cedar remembers, and record sets.
 */
@Serializable
data class Found(
    val conversations: List<Conversation> = emptyList(),
    val agents: List<Agent> = emptyList(),
    val memories: List<OutcomeMemory> = emptyList(),
    val records: List<FoundRecordSet> = emptyList(),
) {
    val isEmpty: Boolean get() = conversations.isEmpty() && agents.isEmpty() && memories.isEmpty() && records.isEmpty()
}
