@file:UseSerializers(InstantSerializer::class)

package dev.cedar.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.UseSerializers
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.time.Instant

// Handing work over from the agent to the person (chat/README.md, "The outbox" and "Feedback";
// chat/src/api/handover.rs). Both follow the rule the follow-up suggestions set: the model
// proposes, the app shows a card, and nothing happens until the person presses a button.

/**
 * Something the model reported it could not do, with `report_limitation`. The tool records
 * nothing; the person sends it as feedback, which stays on their server, or dismisses it.
 */
data class LimitationReport(
    val summary: String,
    /** One of [CATEGORIES]. */
    val category: String,
    /** What would make it possible, when the model said. */
    val needed: String? = null,
) {
    companion object {
        const val TOOL = "report_limitation"
        const val OTHER = "other"
        val CATEGORIES = listOf("missing_tool", "not_connected", "blocked", "limit", OTHER)

        /** Parses the tool's JSON result. `null` when it holds no report with a summary. */
        fun parse(output: String): LimitationReport? {
            val value = parseJson(output)["limitation"] ?: return null
            val summary = value["summary"].str?.trim()?.takeIf { it.isNotEmpty() } ?: return null
            val category = value["category"].str?.takeIf { it in CATEGORIES } ?: OTHER
            return LimitationReport(summary, category, value["needed"].str?.trim()?.takeIf { it.isNotEmpty() })
        }
    }
}

/**
 * What a tool call's result says when the call was not made but kept in the outbox: it would
 * have sent or changed something in one of the person's accounts, and nobody was there to
 * approve it (`unattended`) or nobody answered in time (`timeout`). [id] names it in `/api/outbox`.
 */
data class HeldNote(val id: String, val tool: String, val reason: String, val why: String) {
    val timedOut: Boolean get() = why == "timeout"

    companion object {
        /** Parses a tool result. `null` unless it is the note cedar leaves for a held call. */
        fun parse(output: String): HeldNote? {
            // Any tool's result may be one, so look before parsing what may be a long page.
            if (!output.trimStart().startsWith("{\"held\"")) return null
            val value = parseJson(output)["held"] ?: return null
            val id = value["id"].str?.takeIf { it.isNotEmpty() } ?: return null
            return HeldNote(id, value["tool"].str ?: "", value["reason"].str ?: "", value["why"].str ?: "unattended")
        }
    }
}

/**
 * What the person decided about a report (`limitations` of a conversation, and the answer to
 * deciding): `sent`, kept on the server as feedback, or `dismissed`, which keeps nothing of what
 * it said. A report without one is still open.
 */
@Serializable
data class Feedback(
    val id: String = "",
    @SerialName("turn_id") val turnId: String? = null,
    /** The id of the `report_limitation` tool call, as the transcript has it. */
    @SerialName("call_id") val callId: String = "",
    val status: String = "",
    val summary: String? = null,
    val category: String? = null,
    val needed: String? = null,
    val comment: String? = null,
    @SerialName("created_at") val createdAt: Instant? = null,
) {
    val isSent: Boolean get() = status == SENT
    val isDismissed: Boolean get() = status == DISMISSED

    companion object {
        const val SENT = "sent"
        const val DISMISSED = "dismissed"
    }
}

/** `PUT /api/turns/{turn_id}/limitations/{call_id}`: the decision that stands. */
@Serializable
data class FeedbackDecision(val feedback: Feedback)

/** What the person decides about a report. */
sealed interface FeedbackChoice {
    data object Dismiss : FeedbackChoice

    /** Keep it as feedback: the report as it was shown, and what the person added. */
    data class Send(val report: LimitationReport, val comment: String? = null) : FeedbackChoice

    fun toJson(): JsonObject = when (this) {
        Dismiss -> buildJsonObject { put("decision", "dismiss") }
        is Send -> buildJsonObject {
            put("decision", "send")
            put("summary", report.summary)
            put("category", report.category)
            report.needed?.let { put("needed", it) }
            comment?.trim()?.takeIf { it.isNotEmpty() }?.let { put("comment", it) }
        }
    }
}

/** The agent whose run wanted to make a held call. */
@Serializable
data class OutboxAgent(val id: String = "", val title: String = "")

/**
 * A call that waits in the outbox, or what became of one (`/api/outbox`): the tool and its input
 * exactly as the model wrote them, a line on what it would do, and where it came from when the
 * run is one of this chat's.
 */
@Serializable
data class OutboxItem(
    val id: String,
    @SerialName("run_id") val runId: String = "",
    val tool: String = "",
    val input: JsonObject = JsonObject(emptyMap()),
    val reason: String = "",
    /** `unattended` or `timeout`. */
    val why: String = "",
    /** `pending`, `sending`, `sent`, `failed` (it can be approved again) or `discarded`. */
    val status: String = "",
    /** Whether the person changed the input before it went out. */
    val edited: Boolean = false,
    /** What the tool answered, or why it failed. */
    val output: String? = null,
    @SerialName("created_at") val createdAt: Instant? = null,
    @SerialName("decided_at") val decidedAt: Instant? = null,
    @SerialName("conversation_id") val conversationId: String? = null,
    /** The turn whose run held it, when the run is one of this chat's: its outcome shows the call. */
    @SerialName("turn_id") val turnId: String? = null,
    val title: String? = null,
    val agent: OutboxAgent? = null,
) {
    /** Still to be decided: waiting, or tried and refused by the service. */
    val isOpen: Boolean get() = status == PENDING || status == FAILED
    val isFailed: Boolean get() = status == FAILED
    val isSent: Boolean get() = status == SENT
    val timedOut: Boolean get() = why == "timeout"

    /** Who wanted it: the agent, else the conversation's title. */
    val origin: String? get() = agent?.title?.takeIf { it.isNotEmpty() } ?: title?.takeIf { it.isNotEmpty() }

    /** The input as rows to show: text as it is, anything else as JSON, in the input's order. */
    val fields: List<Pair<String, String>>
        get() = input.entries.map { (name, value) ->
            name to ((value as? JsonPrimitive)?.takeIf { it.isString }?.content ?: value.toString())
        }

    companion object {
        const val PENDING = "pending"
        const val FAILED = "failed"
        const val SENT = "sent"
        const val DISCARDED = "discarded"
    }
}

/** `GET /api/outbox`: what waits for the person's approval, newest first. */
@Serializable
data class Outbox(val items: List<OutboxItem> = emptyList())
