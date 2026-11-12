@file:UseSerializers(InstantSerializer::class)

package dev.cedar.core

import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.descriptors.SerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonDecoder
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonEncoder
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.time.Instant

// Agents (chat/README.md, "Agents"; chat/src/api/agents.rs): a prompt and the trigger that starts
// its runs. The shapes `/api/agents` returns and takes, and what the app works out from them
// without Android: which group a trigger belongs to, what an agent is doing right now, and whether
// a trigger being edited is one the server will take.

/** Where the list puts an agent, by what starts its runs. */
enum class TriggerGroup { SCHEDULE, CONDITION, WEBHOOK, MAIL_CALENDAR, OTHER }

/** The triggers the app can make and edit; the server's `kind`. */
enum class TriggerKind(val id: String, val group: TriggerGroup) {
    TIME("time", TriggerGroup.SCHEDULE),
    ONCE("once", TriggerGroup.SCHEDULE),
    INTERVAL("interval", TriggerGroup.SCHEDULE),
    MONTHLY("monthly", TriggerGroup.SCHEDULE),
    CONDITION("condition", TriggerGroup.CONDITION),
    WEBHOOK("webhook", TriggerGroup.WEBHOOK),
    EMAIL("email", TriggerGroup.MAIL_CALENDAR),
    CALENDAR("calendar", TriggerGroup.MAIL_CALENDAR);

    /** The scheduler checks something for it between runs (it can be tried out). */
    val watches: Boolean get() = this == CONDITION || this == EMAIL || this == CALENDAR

    /** Something outside starts its runs, not the clock: these have limits and a careful choice of tools. */
    val byEvent: Boolean get() = watches || this == WEBHOOK

    companion object {
        fun of(id: String): TriggerKind? = entries.firstOrNull { it.id == id }
    }
}

/**
 * What starts an agent's runs, by `kind`. A kind this app doesn't know (a newer server's) is
 * [Unknown] and keeps its JSON: it is listed and shown, and never sent back.
 */
@Serializable(with = TriggerSerializer::class)
sealed interface Trigger {
    /** The server's name for it. */
    val kind: String

    /** Every day, on weekdays, or weekly on `weekday` (0 = Monday), at `time` (`HH:MM`) in `timezone`. */
    data class Time(val frequency: String, val weekday: Int? = null, val time: String, override val timezone: String) : Trigger {
        override val kind: String get() = "time"
    }

    /** One run at `at` (`2026-12-03T09:00`, local to `timezone`); the agent switches itself off after. */
    data class Once(val at: String, override val timezone: String) : Trigger {
        override val kind: String get() = "once"
    }

    data class Interval(val everyMinutes: Int) : Trigger {
        override val kind: String get() = "interval"
    }

    /** On `day` of every month (the last day in a shorter month) at `time` in `timezone`. */
    data class Monthly(val day: Int, val time: String, override val timezone: String) : Trigger {
        override val kind: String get() = "monthly"
    }

    /** A run when what `source` returns (the value at `extract`, if given) passes `predicate`. */
    data class Condition(
        val source: WatchSource,
        val extract: String? = null,
        val predicate: Predicate,
        val everyMinutes: Int = AgentLimits.DEFAULT_CHECK_MINUTES,
    ) : Trigger {
        override val kind: String get() = "condition"
    }

    /** A run when the agent's address is called with its secret. */
    data object Webhook : Trigger {
        override val kind: String get() = "webhook"
    }

    /** A run when mail matching `query` (Gmail's search syntax; empty for any mail) arrives. */
    data class Email(val query: String = "", val everyMinutes: Int = AgentLimits.DEFAULT_CHECK_MINUTES) : Trigger {
        override val kind: String get() = "email"
    }

    /** A run `minutesBefore` an event starts, when a new event appears, or both. */
    data class Calendar(
        val calendarId: String? = null,
        val filter: String? = null,
        val minutesBefore: Int? = null,
        val newEvents: Boolean = false,
        val everyMinutes: Int = AgentLimits.DEFAULT_CHECK_MINUTES,
    ) : Trigger {
        override val kind: String get() = "calendar"
    }

    /** A kind from a newer server, or one of ours that doesn't read as it should. */
    data class Unknown(override val kind: String, val raw: JsonObject = JsonObject(emptyMap())) : Trigger

    val group: TriggerGroup get() = TriggerKind.of(kind)?.takeIf { this !is Unknown }?.group ?: TriggerGroup.OTHER

    /** A clock starts its runs: it has a next run. */
    val isClock: Boolean get() = this is Time || this is Once || this is Interval || this is Monthly

    val watches: Boolean get() = this is Condition || this is Email || this is Calendar

    val byEvent: Boolean get() = watches || this is Webhook

    /** The time zone its times are in, for the triggers that have one. */
    val timezone: String? get() = null

    /** How often it checks, for the triggers that watch. */
    val checkMinutes: Int?
        get() = when (this) {
            is Condition -> everyMinutes
            is Email -> everyMinutes
            is Calendar -> everyMinutes
            else -> null
        }

    /** Exactly what the server takes: absent optionals are left out (it refuses fields it doesn't know). */
    fun toJson(): JsonObject = when (this) {
        is Time -> buildJsonObject {
            put("kind", kind)
            put("frequency", frequency)
            weekday?.let { put("weekday", it) }
            put("time", time)
            put("timezone", timezone)
        }
        is Once -> buildJsonObject {
            put("kind", kind)
            put("at", at)
            put("timezone", timezone)
        }
        is Interval -> buildJsonObject {
            put("kind", kind)
            put("every_minutes", everyMinutes)
        }
        is Monthly -> buildJsonObject {
            put("kind", kind)
            put("day", day)
            put("time", time)
            put("timezone", timezone)
        }
        is Condition -> buildJsonObject {
            put("kind", kind)
            put("source", source.toJson())
            extract?.let { put("extract", it) }
            put("predicate", predicate.toJson())
            put("every_minutes", everyMinutes)
        }
        Webhook -> buildJsonObject { put("kind", kind) }
        is Email -> buildJsonObject {
            put("kind", kind)
            put("query", query)
            put("every_minutes", everyMinutes)
        }
        is Calendar -> buildJsonObject {
            put("kind", kind)
            calendarId?.let { put("calendar_id", it) }
            filter?.let { put("filter", it) }
            minutesBefore?.let { put("minutes_before", it) }
            put("new_events", newEvents)
            put("every_minutes", everyMinutes)
        }
        is Unknown -> raw
    }

    companion object {
        /** Reads a trigger; whatever is there, the answer is a trigger (an [Unknown] one if need be). */
        fun fromJson(json: JsonElement?): Trigger {
            val o = json as? JsonObject ?: return Unknown("")
            val kind = o["kind"].str.orEmpty()
            val every = o["every_minutes"].int ?: AgentLimits.DEFAULT_CHECK_MINUTES
            val known: Trigger? = when (kind) {
                "time" -> {
                    val (frequency, time, timezone) = Triple(o["frequency"].str, o["time"].str, o["timezone"].str)
                    if (frequency != null && time != null && timezone != null) Time(frequency, o["weekday"].int, time, timezone) else null
                }
                "once" -> {
                    val (at, timezone) = o["at"].str to o["timezone"].str
                    if (at != null && timezone != null) Once(at, timezone) else null
                }
                "interval" -> o["every_minutes"].int?.let { Interval(it) }
                "monthly" -> {
                    val (day, time, timezone) = Triple(o["day"].int, o["time"].str, o["timezone"].str)
                    if (day != null && time != null && timezone != null) Monthly(day, time, timezone) else null
                }
                "condition" -> {
                    val source = WatchSource.fromJson(o["source"])
                    val op = o["predicate"]["op"].str
                    if (source != null && op != null) {
                        Condition(source, o["extract"].str, Predicate(op, o["predicate"]["value"]?.takeIf { it !is JsonNull }), every)
                    } else {
                        null
                    }
                }
                "webhook" -> Webhook
                "email" -> Email(o["query"].str.orEmpty(), every)
                "calendar" -> Calendar(o["calendar_id"].str, o["filter"].str, o["minutes_before"].int, o["new_events"].bool ?: false, every)
                else -> null
            }
            return known ?: Unknown(kind, o)
        }
    }
}

/** Triggers as the JSON objects the server writes and reads; decoding never fails (see [Trigger.fromJson]). */
object TriggerSerializer : KSerializer<Trigger> {
    override val descriptor: SerialDescriptor = JsonElement.serializer().descriptor

    override fun deserialize(decoder: Decoder): Trigger {
        val json = (decoder as? JsonDecoder)?.decodeJsonElement() ?: throw IllegalStateException("triggers are read from JSON only")
        return Trigger.fromJson(json)
    }

    override fun serialize(encoder: Encoder, value: Trigger) {
        (encoder as? JsonEncoder)?.encodeJsonElement(value.toJson()) ?: throw IllegalStateException("triggers are written as JSON only")
    }
}

/** What a condition watches. */
sealed interface WatchSource {
    /** A web address: a JSON API's reply (`format` `json`), a page's text (`text`), or whichever it turns out to be (null). */
    data class Url(val url: String, val format: String? = null) : WatchSource

    /** One of cedar's tools that can be called outside a run (`pollable` in the catalog), with this input. */
    data class Tool(val name: String, val input: JsonObject = JsonObject(emptyMap())) : WatchSource

    /** One of the record sets cedar keeps, seen as `{count, updated_at, records}`. */
    data class Records(val set: String) : WatchSource

    /** The server calls a URL source's format `as`. */
    fun toJson(): JsonObject = when (this) {
        is Url -> buildJsonObject {
            put("type", "url")
            put("url", url)
            format?.let { put("as", it) }
        }
        is Tool -> buildJsonObject {
            put("type", "tool")
            put("name", name)
            put("input", input)
        }
        is Records -> buildJsonObject {
            put("type", "records")
            put("set", set)
        }
    }

    companion object {
        /** `null` for a source this app doesn't know; its trigger is then shown as it came. */
        fun fromJson(json: JsonElement?): WatchSource? = when (json["type"].str) {
            "url" -> json["url"].str?.let { Url(it, json["as"].str) }
            "tool" -> json["name"].str?.let { Tool(it, json["input"] as? JsonObject ?: JsonObject(emptyMap())) }
            "records" -> json["set"].str?.let { Records(it) }
            else -> null
        }
    }
}

/**
 * The test a watched value has to pass: `changes` (no value), `gt` `lt` `gte` `lte` (a number), or
 * `equals` `contains` (a text, a number, or true or false).
 */
data class Predicate(val op: String, val value: JsonElement? = null) {
    val needsValue: Boolean get() = op != CHANGES
    val needsNumber: Boolean get() = op in THRESHOLDS

    /** The value as a person would write it: text without its quotes, 40 and not 40.0. */
    val valueText: String
        get() = when (val v = value) {
            null, is JsonNull -> ""
            is JsonPrimitive -> if (v.isString) v.content else v.content.removeSuffix(".0")
            else -> v.toString()
        }

    fun toJson(): JsonObject = buildJsonObject {
        put("op", op)
        value?.let { put("value", it) }
    }

    companion object {
        const val CHANGES = "changes"
        val THRESHOLDS = listOf("gt", "gte", "lt", "lte")

        /** Every test, in the order the form offers them. */
        val OPS = listOf(CHANGES) + THRESHOLDS + listOf("equals", "contains")

        /** The predicate for what was typed: a number for a threshold, the text itself otherwise; `null` if it won't do. */
        fun of(op: String, text: String): Predicate? {
            val trimmed = text.trim()
            return when {
                op == CHANGES -> Predicate(op)
                op in THRESHOLDS -> trimmed.replace(',', '.').toDoubleOrNull()?.takeIf { it.isFinite() }?.let { n ->
                    // 40, not 40.0: a whole number is sent as one.
                    Predicate(op, if (n % 1.0 == 0.0 && kotlin.math.abs(n) < 1e15) JsonPrimitive(n.toLong()) else JsonPrimitive(n))
                }
                trimmed.isEmpty() -> null
                else -> Predicate(op, JsonPrimitive(trimmed))
            }
        }
    }
}

/** The agent a run or a conversation belongs to, where only that much is sent. */
@Serializable
data class AgentRef(
    val id: String,
    val title: String = "",
    @SerialName("trigger_kind") val triggerKind: String = "",
)

/** A webhook agent's address. `secret` is there only in the answer to creating it or to a new secret. */
@Serializable
data class Hook(
    val url: String = "",
    val path: String = "",
    @SerialName("secret_last4") val secretLast4: String? = null,
    val secret: String? = null,
) {
    /**
     * A call that starts a run, to try from a shell. Without the secret (it is shown once), a
     * placeholder stands in for it.
     */
    fun curlExample(secret: String? = this.secret): String =
        "curl -X POST -H \"Authorization: Bearer ${secret ?: "<secret>"}\" -H \"content-type: application/json\" -d '{\"door\":\"open\"}' $url"
}

/**
 * An agent (`GET /api/agents`): a prompt the server starts as a fresh run whenever its trigger
 * says so, each run in a conversation of its own that the conversation list leaves out. Only `id`
 * and `title` are sure to be there.
 */
@Serializable
data class Agent(
    val id: String,
    val title: String,
    val prompt: String = "",
    val model: String? = null,
    val reasoning: String? = null,
    /** The tools its runs use (tool and group names); `null` for cedar's defaults. */
    val tools: List<String>? = null,
    /** `none`, a method's name, or `null` to follow the default in the server's Settings. */
    val notify: String? = null,
    val enabled: Boolean = true,
    @SerialName("source_conversation_id") val sourceConversationId: String? = null,
    val timezone: String? = null,
    val trigger: Trigger = Trigger.Unknown(""),
    /** Clock triggers only; `null` for the others and for a one-off that has run. */
    @SerialName("next_run_at") val nextRunAt: Instant? = null,
    /** Watching triggers only. */
    @SerialName("next_check_at") val nextCheckAt: Instant? = null,
    @SerialName("last_run_at") val lastRunAt: Instant? = null,
    @SerialName("last_checked_at") val lastCheckedAt: Instant? = null,
    /** Why the last check failed. */
    @SerialName("last_error") val lastError: String? = null,
    /** A webhook's call is held until the agent may run again. */
    val waiting: Boolean = false,
    /** How far apart events may start runs, and how many in a day; `null` for the list's `limits`. */
    @SerialName("min_interval_secs") val minIntervalSecs: Int? = null,
    @SerialName("max_runs_per_day") val maxRunsPerDay: Int? = null,
    val hook: Hook? = null,
    @SerialName("last_run") val lastRun: AgentRun? = null,
    @SerialName("created_at") val createdAt: Instant? = null,
    @SerialName("updated_at") val updatedAt: Instant? = null,
) {
    /** What the agent is doing, for its row. */
    sealed interface Status {
        data object Paused : Status

        /** A one-off that has run. */
        data object Done : Status

        /** The last check failed; it is tried again. */
        data class CheckFailing(val error: String) : Status

        /** A clock: when it runs next, if the server said. */
        data class NextRun(val at: Instant?) : Status

        /** It watches something: when it last looked, if it has. */
        data class Watching(val lastCheckedAt: Instant?) : Status

        /** A webhook nobody is calling right now. */
        data object WaitingForCalls : Status

        /** A webhook whose call is held until the agent may run. */
        data object CallWaiting : Status

        /** A trigger this app doesn't know, switched on. */
        data object Active : Status
    }

    val status: Status
        get() = when {
            !enabled -> if (trigger is Trigger.Once && nextRunAt == null) Status.Done else Status.Paused
            lastError != null -> Status.CheckFailing(lastError)
            trigger.isClock -> Status.NextRun(nextRunAt)
            trigger.watches -> Status.Watching(lastCheckedAt)
            trigger is Trigger.Webhook -> if (waiting) Status.CallWaiting else Status.WaitingForCalls
            else -> Status.Active
        }

    /** The trigger's time zone, when it has one and it isn't this device's. */
    fun zoneLabel(deviceZone: String = java.util.TimeZone.getDefault().id): String? = trigger.timezone?.takeIf { it != deviceZone }
}

/** What started a run: `kind` is `clock`, `changed`, `holds`, `webhook`, `mail`, `soon` or `new_event`. */
@Serializable
data class RunCause(
    val kind: String = "",
    /** One line, already in the user's language. */
    val summary: String = "",
    /** The start of the data that came with it. */
    val excerpt: String? = null,
)

/**
 * One time an agent started, with the turn that carries its run, or the `error` that kept it from
 * starting. The activity feed adds the `agent` it belongs to.
 */
@Serializable
data class AgentRun(
    val id: String,
    /** The agent's id. */
    @SerialName("schedule_id") val agentId: String = "",
    @SerialName("conversation_id") val conversationId: String? = null,
    @SerialName("turn_id") val turnId: String? = null,
    @SerialName("due_at") val dueAt: Instant? = null,
    @SerialName("started_at") val startedAt: Instant? = null,
    /** Started with Run now rather than by its trigger. */
    val manual: Boolean = false,
    val error: String? = null,
    /** `null` for Run now, and for runs from before triggers. */
    val cause: RunCause? = null,
    val turn: Turn? = null,
    val agent: AgentRef? = null,
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

    /** When it started, or was due. */
    val time: Instant? get() = startedAt ?: dueAt

    /** What started it, in the server's words; `null` for Run now and for runs from before. */
    val startedBy: String? get() = cause?.summary?.trim()?.takeIf { it.isNotEmpty() }
}

/** What the server allows and assumes for agents (`limits` of `GET /api/agents`). */
@Serializable
data class AgentLimits(
    @SerialName("max_agents") val maxAgents: Int = 100,
    /** How far apart events may start an agent's runs unless it says. */
    @SerialName("min_interval_secs") val minIntervalSecs: Int = 300,
    @SerialName("max_runs_per_day") val maxRunsPerDay: Int = 24,
    /** The shortest interval trigger. */
    @SerialName("min_interval_minutes") val minIntervalMinutes: Int = 15,
    /** How often a watching trigger checks unless it says. */
    @SerialName("check_minutes") val checkMinutes: Int = DEFAULT_CHECK_MINUTES,
    /** A personal tool is checked no more often than this. */
    @SerialName("personal_check_minutes") val personalCheckMinutes: Int = 5,
) {
    companion object {
        const val DEFAULT_CHECK_MINUTES = 5

        /** The longest interval, the longest time between two checks, and the longest notice of an event: a week. */
        const val MAX_EVERY_MINUTES = 7 * 24 * 60
        val MIN_INTERVAL_SECS_RANGE = 60..7 * 24 * 3600
        val MAX_RUNS_PER_DAY_RANGE = 1..500
    }
}

/** `GET /api/agents`: the agents, enabled first, and the limits. */
@Serializable
data class AgentList(val agents: List<Agent> = emptyList(), val limits: AgentLimits = AgentLimits())

/** `GET /api/agents/{id}`: the agent and its runs, newest first. */
@Serializable
data class AgentDetail(val agent: Agent, val runs: List<AgentRun> = emptyList())

/** `GET /api/agents/activity`: the latest runs of every agent, newest first; `nextBefore` asks for the page after. */
@Serializable
data class ActivityPage(
    val items: List<AgentRun> = emptyList(),
    @SerialName("next_before") val nextBefore: String? = null,
)

/**
 * What trying a trigger out found (`POST /api/agents/test`): the value seen and whether a run
 * would start, or the `error`; and the start of what the check returned, to find a path in.
 */
@Serializable
data class TriggerTest(
    val ok: Boolean = false,
    val value: JsonElement? = null,
    @SerialName("would_fire") val wouldFire: Boolean = false,
    /** What would start the run, in the user's language. */
    val summary: String? = null,
    val error: String? = null,
    val sample: Sample? = null,
) {
    @Serializable
    data class Sample(
        val text: String = "",
        /** There was more than `text` shows. */
        val cut: Boolean = false,
        /** `text` is JSON, pretty-printed. */
        val json: Boolean = false,
    )

    /** The value as a person would write it. */
    val valueText: String
        get() = when (val v = value) {
            null, is JsonNull -> ""
            is JsonPrimitive -> v.content
            else -> v.pretty()
        }
}

/** `POST /api/conversations/{id}/compact`: the conversation as one prompt, for making an agent of it. */
@Serializable
data class Compacted(val prompt: String = "", val source: String = "")

/** One tool of cedar's catalog (`GET /api/tools`). */
@Serializable
data class ToolInfo(
    val name: String,
    val description: String = "",
    /** It changes something outside (sends mail, say). */
    val writes: Boolean = false,
    /** It can be called outside a run, so a condition can watch it. */
    val pollable: Boolean = false,
)

/** A group of tools; `custom` holds the personal and community tools. */
@Serializable
data class ToolGroup(
    val id: String,
    @SerialName("label") private val rawLabel: String? = null,
    val available: Boolean = true,
    val tools: List<ToolInfo> = emptyList(),
    val discovered: Boolean? = null,
    val reason: String? = null,
) {
    val label: String get() = rawLabel?.takeIf { it.isNotBlank() } ?: id
    val isPersonal: Boolean get() = id == PERSONAL

    companion object {
        const val PERSONAL = "custom"
    }
}

@Serializable
data class ToolCatalog(val groups: List<ToolGroup> = emptyList()) {
    /** The tools a condition can watch, with their groups. */
    val pollable: List<Pair<ToolGroup, ToolInfo>>
        get() = groups.flatMap { group -> group.tools.filter { it.pollable }.map { group to it } }

    /** Whether `name` is a personal tool: its code then runs unattended on every check. */
    fun isPersonal(name: String): Boolean = groups.any { it.isPersonal && it.tools.any { tool -> tool.name == name } }
}

/** What `GET /api/notifications` says that the agent form needs: the default method and the ones set up. */
@Serializable
data class NotificationStatus(
    @SerialName("default_method") val defaultMethod: String = "none",
    val configured: List<String> = emptyList(),
)

/** A field of a request that can be left as it is, cleared (`null`), or set. */
sealed interface Change<out T> {
    data object Keep : Change<Nothing>
    data object Clear : Change<Nothing>
    data class Set<T>(val value: T) : Change<T>
}

/**
 * The body of `POST /api/agents` and `PATCH /api/agents/{id}`: what is `null` or [Change.Keep] is
 * left out (the server refuses fields it doesn't know and tells absent from `null`).
 */
data class AgentBody(
    val title: String? = null,
    val prompt: String? = null,
    /** A model's id; "" for cedar's default. */
    val model: String? = null,
    val reasoning: String? = null,
    val enabled: Boolean? = null,
    /** The chat the agent is made from. */
    val conversationId: String? = null,
    /** Left out when creating, an event-started agent gets a careful selection; `Clear` means cedar's defaults. */
    val tools: Change<List<String>> = Change.Keep,
    val notify: Change<String> = Change.Keep,
    val trigger: Trigger? = null,
    /** The device's IANA time zone, for the triggers that carry none. */
    val timezone: String? = null,
    val minIntervalSecs: Change<Int> = Change.Keep,
    val maxRunsPerDay: Change<Int> = Change.Keep,
) {
    fun toJson(): JsonObject = buildJsonObject {
        title?.let { put("title", it) }
        prompt?.let { put("prompt", it) }
        model?.let { put("model", it) }
        reasoning?.let { put("reasoning", it) }
        enabled?.let { put("enabled", it) }
        conversationId?.let { put("conversation_id", it) }
        when (tools) {
            Change.Keep -> {}
            Change.Clear -> put("tools", JsonNull)
            is Change.Set -> put("tools", JsonArray(tools.value.map { JsonPrimitive(it) }))
        }
        when (notify) {
            Change.Keep -> {}
            Change.Clear -> put("notify", JsonNull)
            is Change.Set -> put("notify", notify.value)
        }
        trigger?.let { put("trigger", it.toJson()) }
        timezone?.let { put("timezone", it) }
        when (minIntervalSecs) {
            Change.Keep -> {}
            Change.Clear -> put("min_interval_secs", JsonNull)
            is Change.Set -> put("min_interval_secs", minIntervalSecs.value)
        }
        when (maxRunsPerDay) {
            Change.Keep -> {}
            Change.Clear -> put("max_runs_per_day", JsonNull)
            is Change.Set -> put("max_runs_per_day", maxRunsPerDay.value)
        }
    }
}

// -- the trigger being edited ---------------------------------------------------------------

/** Why a trigger being edited can't be saved yet; the app words each in the user's language. */
sealed interface TriggerProblem {
    /** The time isn't `HH:MM`. */
    data object Time : TriggerProblem

    /** The date isn't `YYYY-MM-DD`. */
    data object Date : TriggerProblem

    /** A one-off's time has passed. */
    data object InThePast : TriggerProblem

    /** The day of the month isn't 1 to 31. */
    data object Day : TriggerProblem

    /** How often it runs or checks is outside `min..max` minutes. */
    data class Every(val min: Int, val max: Int) : TriggerProblem

    /** The address doesn't start with `https://` or `http://`. */
    data object Url : TriggerProblem

    /** No tool is picked. */
    data object Tool : TriggerProblem

    /** The tool's input isn't a JSON object. */
    data object ToolInput : TriggerProblem

    /** No record set is picked. */
    data object RecordSet : TriggerProblem

    /** A threshold needs a number. */
    data object Number : TriggerProblem

    /** Equals and contains need something to compare with. */
    data object Value : TriggerProblem

    /** A calendar trigger needs a notice before events, new events, or both. */
    data object CalendarNeedsOne : TriggerProblem

    /** The notice before an event is outside `0..max` minutes. */
    data class MinutesBefore(val max: Int) : TriggerProblem
}

/** Minutes, as the form asks for them: a number and its unit. */
enum class Span(val minutes: Int) {
    MINUTES(1), HOURS(60), DAYS(24 * 60);

    companion object {
        /** `90` → 90 minutes, `120` → 2 hours, `2880` → 2 days. */
        fun split(minutes: Int): Pair<Int, Span> = when {
            minutes > 0 && minutes % DAYS.minutes == 0 -> minutes / DAYS.minutes to DAYS
            minutes > 0 && minutes % HOURS.minutes == 0 -> minutes / HOURS.minutes to HOURS
            else -> minutes to MINUTES
        }
    }
}

/**
 * A trigger as the form holds it: every kind's fields at once, as typed, so that switching kinds
 * loses nothing. [build] makes the trigger the server takes, or says what is missing.
 */
data class TriggerDraft(
    val kind: TriggerKind = TriggerKind.TIME,
    /** The zone the times are in: the device's for a new agent, the trigger's own when editing. */
    val timezone: String,
    // Time, once and monthly share the time of day.
    val frequency: String = "daily",
    val weekday: Int = 0,
    val time: String = "08:00",
    /** A one-off's day, `YYYY-MM-DD`. */
    val date: String = "",
    val every: String = "1",
    val everyUnit: Span = Span.HOURS,
    val day: String = "1",
    // Condition
    val sourceType: String = SOURCE_URL,
    val url: String = "",
    /** `json`, `text`, or "" to let the server tell. */
    val urlFormat: String = "",
    val tool: String = "",
    val toolInput: String = "{}",
    val recordSet: String = "",
    val extract: String = "",
    val op: String = Predicate.CHANGES,
    val value: String = "",
    // What watches: how often it checks.
    val check: String = AgentLimits.DEFAULT_CHECK_MINUTES.toString(),
    val checkUnit: Span = Span.MINUTES,
    // Email
    val query: String = "",
    // Calendar
    val calendarId: String? = null,
    val filter: String = "",
    val beforeEvents: Boolean = true,
    val minutesBefore: String = "30",
    val newEvents: Boolean = false,
) {
    sealed interface Built {
        data class Ready(val trigger: Trigger) : Built
        data class Invalid(val problem: TriggerProblem) : Built
    }

    /**
     * The trigger, or the first thing wrong with it. `personalTool` says the tool picked is a
     * personal one (checked no more often than the limits say); `now` is the local time in
     * [timezone], to refuse a one-off in the past (left out, that is the server's to say).
     */
    fun build(limits: AgentLimits = AgentLimits(), personalTool: Boolean = false, now: java.time.LocalDateTime? = null): Built {
        fun invalid(problem: TriggerProblem) = Built.Invalid(problem)
        val clock = clock(time)
        fun checkEvery(least: Int): Int? = minutes(check, checkUnit)?.takeIf { it in least..AgentLimits.MAX_EVERY_MINUTES }
        return when (kind) {
            TriggerKind.TIME -> {
                clock ?: return invalid(TriggerProblem.Time)
                Built.Ready(Trigger.Time(frequency, weekday.takeIf { frequency == "weekly" }?.coerceIn(0, 6), clock, timezone))
            }
            TriggerKind.ONCE -> {
                clock ?: return invalid(TriggerProblem.Time)
                val day = runCatching { java.time.LocalDate.parse(date.trim()) }.getOrNull() ?: return invalid(TriggerProblem.Date)
                val at = day.atTime(java.time.LocalTime.parse(clock))
                if (now != null && !at.isAfter(now)) return invalid(TriggerProblem.InThePast)
                Built.Ready(Trigger.Once("${day}T$clock", timezone))
            }
            TriggerKind.INTERVAL -> {
                val n = minutes(every, everyUnit)?.takeIf { it in limits.minIntervalMinutes..AgentLimits.MAX_EVERY_MINUTES }
                    ?: return invalid(TriggerProblem.Every(limits.minIntervalMinutes, AgentLimits.MAX_EVERY_MINUTES))
                Built.Ready(Trigger.Interval(n))
            }
            TriggerKind.MONTHLY -> {
                val d = day.trim().toIntOrNull()?.takeIf { it in 1..31 } ?: return invalid(TriggerProblem.Day)
                clock ?: return invalid(TriggerProblem.Time)
                Built.Ready(Trigger.Monthly(d, clock, timezone))
            }
            TriggerKind.CONDITION -> {
                val source = when (sourceType) {
                    SOURCE_TOOL -> {
                        if (tool.isBlank()) return invalid(TriggerProblem.Tool)
                        val input = toolInput.trim().ifEmpty { "{}" }.let(::parseJson) as? JsonObject ?: return invalid(TriggerProblem.ToolInput)
                        WatchSource.Tool(tool.trim(), input)
                    }
                    SOURCE_RECORDS -> {
                        if (recordSet.isBlank()) return invalid(TriggerProblem.RecordSet)
                        WatchSource.Records(recordSet.trim())
                    }
                    else -> {
                        val address = url.trim()
                        if (!(address.startsWith("https://") || address.startsWith("http://")) || address.length < 9) return invalid(TriggerProblem.Url)
                        WatchSource.Url(address, urlFormat.takeIf { it == "json" || it == "text" })
                    }
                }
                val predicate = Predicate.of(op, value)
                    ?: return invalid(if (op in Predicate.THRESHOLDS) TriggerProblem.Number else TriggerProblem.Value)
                val least = if (sourceType == SOURCE_TOOL && personalTool) limits.personalCheckMinutes else 1
                val n = checkEvery(least) ?: return invalid(TriggerProblem.Every(least, AgentLimits.MAX_EVERY_MINUTES))
                Built.Ready(Trigger.Condition(source, extract.trim().ifEmpty { null }, predicate, n))
            }
            TriggerKind.WEBHOOK -> Built.Ready(Trigger.Webhook)
            TriggerKind.EMAIL -> {
                val n = checkEvery(1) ?: return invalid(TriggerProblem.Every(1, AgentLimits.MAX_EVERY_MINUTES))
                Built.Ready(Trigger.Email(query.trim(), n))
            }
            TriggerKind.CALENDAR -> {
                if (!beforeEvents && !newEvents) return invalid(TriggerProblem.CalendarNeedsOne)
                val before = if (beforeEvents) {
                    minutesBefore.trim().toIntOrNull()?.takeIf { it in 0..AgentLimits.MAX_EVERY_MINUTES }
                        ?: return invalid(TriggerProblem.MinutesBefore(AgentLimits.MAX_EVERY_MINUTES))
                } else {
                    null
                }
                val n = checkEvery(1) ?: return invalid(TriggerProblem.Every(1, AgentLimits.MAX_EVERY_MINUTES))
                Built.Ready(Trigger.Calendar(calendarId?.trim()?.ifEmpty { null }, filter.trim().ifEmpty { null }, before, newEvents, n))
            }
        }
    }

    companion object {
        const val SOURCE_URL = "url"
        const val SOURCE_TOOL = "tool"
        const val SOURCE_RECORDS = "records"
        val FREQUENCIES = listOf("daily", "weekdays", "weekly")

        /** `7:30` or `07:30` as the server's `HH:MM`; `null` for anything else. */
        fun clock(text: String): String? {
            val match = Regex("""(\d{1,2}):(\d{2})""").matchEntire(text.trim()) ?: return null
            val (hour, minute) = match.groupValues[1].toInt() to match.groupValues[2].toInt()
            if (hour > 23 || minute > 59) return null
            return hour.toString().padStart(2, '0') + ":" + minute.toString().padStart(2, '0')
        }

        private fun minutes(text: String, unit: Span): Int? =
            text.trim().toLongOrNull()?.takeIf { it > 0 }?.let { it * unit.minutes }?.takeIf { it <= Int.MAX_VALUE }?.toInt()

        /**
         * The draft for editing `trigger`, or `null` for one this app can't edit (an unknown kind).
         * `deviceZone` is the zone a kind without one of its own would get, were it switched to.
         */
        fun from(trigger: Trigger, deviceZone: String): TriggerDraft? {
            val base = TriggerDraft(timezone = trigger.timezone ?: deviceZone)
            fun TriggerDraft.checking(minutes: Int): TriggerDraft = Span.split(minutes).let { (n, unit) -> copy(check = n.toString(), checkUnit = unit) }
            return when (trigger) {
                is Trigger.Time -> base.copy(kind = TriggerKind.TIME, frequency = trigger.frequency, weekday = trigger.weekday ?: 0, time = trigger.time)
                is Trigger.Once -> base.copy(kind = TriggerKind.ONCE, date = trigger.at.substringBefore('T'), time = trigger.at.substringAfter('T', "08:00").take(5))
                is Trigger.Interval -> Span.split(trigger.everyMinutes).let { (n, unit) -> base.copy(kind = TriggerKind.INTERVAL, every = n.toString(), everyUnit = unit) }
                is Trigger.Monthly -> base.copy(kind = TriggerKind.MONTHLY, day = trigger.day.toString(), time = trigger.time)
                is Trigger.Condition -> {
                    val sourced = when (val source = trigger.source) {
                        is WatchSource.Url -> base.copy(sourceType = SOURCE_URL, url = source.url, urlFormat = source.format.orEmpty())
                        is WatchSource.Tool -> base.copy(sourceType = SOURCE_TOOL, tool = source.name, toolInput = source.input.pretty())
                        is WatchSource.Records -> base.copy(sourceType = SOURCE_RECORDS, recordSet = source.set)
                    }
                    sourced.copy(
                        kind = TriggerKind.CONDITION,
                        extract = trigger.extract.orEmpty(),
                        op = trigger.predicate.op,
                        value = trigger.predicate.valueText,
                    ).checking(trigger.everyMinutes)
                }
                Trigger.Webhook -> base.copy(kind = TriggerKind.WEBHOOK)
                is Trigger.Email -> base.copy(kind = TriggerKind.EMAIL, query = trigger.query).checking(trigger.everyMinutes)
                is Trigger.Calendar -> base.copy(
                    kind = TriggerKind.CALENDAR,
                    calendarId = trigger.calendarId,
                    filter = trigger.filter.orEmpty(),
                    beforeEvents = trigger.minutesBefore != null,
                    minutesBefore = (trigger.minutesBefore ?: 30).toString(),
                    newEvents = trigger.newEvents,
                ).checking(trigger.everyMinutes)
                is Trigger.Unknown -> null
            }
        }
    }
}
