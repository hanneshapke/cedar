package dev.cedar.core

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject

// The event-to-transcript reducer, as iOS/Cedar/Models/Transcript.swift has it.

data class Usage(
    val inputTokens: Int = 0,
    val outputTokens: Int = 0,
    /** Summed from the `usage` events that carry one (models with a price). */
    val costUSD: Double? = null,
)

/** A page, file or search result the agent read, numbered as the model cites it. */
data class Source(val n: Int, val url: String, val title: String?)

/** A tool call the agent made, and its result once it arrives. */
data class ToolCall(
    val id: String,
    val name: String,
    val input: JsonElement,
    val output: String? = null,
    val isError: Boolean = false,
    val durationMs: Int? = null,
    val sources: List<Source> = emptyList(),
    /** The card the app draws for a finished card tool, parsed once when its result arrives. */
    val card: Card? = null,
    /** Charts any other tool printed (`[chart] {spec}` lines), drawn after it. */
    val charts: List<ChartCard> = emptyList(),
) {
    val isFinished: Boolean get() = output != null
    val isCard: Boolean get() = name in CARD_TOOLS

    /** A one-line description: the command for `bash`, the path for the file tools, a title for the cards. */
    val summary: String
        get() = when (name) {
            "bash" -> input["command"].str ?: ""
            "run_python" -> input["code"].str?.lineSequence()?.firstOrNull { it.isNotEmpty() } ?: ""
            "read_file", "write_file", "read_document", "generate_chart", "generate_pdf", "download_file" ->
                input["path"].str ?: input["url"].str ?: ""
            "fetch_url" -> input["url"].str ?: ""
            "web_search" -> input["query"].str ?: ""
            "remember" -> input["text"].str ?: ""
            "forget" -> ""
            "show_map", "show_recipe", "plan_trip", "compare_stocks", "search_products", "search_flights" -> cardTitle ?: ""
            else -> input.pretty().replace("\n", " ")
        }

    /** What a card is about, from its result when it has one, else its input. */
    val cardTitle: String?
        get() {
            if (name == "search_flights") return flightsRoute
            val result = output?.let(::parseJson)
            if (result != null) {
                result["title"].str?.takeIf { it.isNotEmpty() }?.let { return it }
                if (name == "compare_stocks") {
                    val names = result["symbols"].arr?.mapNotNull { it["symbol"].str }.orEmpty()
                    if (names.isNotEmpty()) return names.joinToString(", ")
                }
                if (name == "plan_trip") {
                    val from = result["from"]["label"].str
                    val to = result["to"]["label"].str
                    if (from != null && to != null) return "$from → $to"
                }
            }
            (input["title"].str ?: input["dish"].str ?: input["query"].str)?.let { return it }
            if (name == "plan_trip") {
                val from = input["from"].str
                val to = input["to"].str
                if (from != null && to != null) return "$from → $to"
            }
            input["symbols"].arr?.let { symbols -> return symbols.mapNotNull { it.str }.joinToString(", ") }
            return null
        }

    /** "SFO → MUC" for a flight search: the route it was asked for, else the one its result names. */
    private val flightsRoute: String?
        get() {
            for (value in listOfNotNull(input, output?.let(::parseJson))) {
                val from = FlightsCard.airports(value["from"])
                val to = FlightsCard.airports(value["to"])
                if (from.isNotEmpty() && to.isNotEmpty()) return FlightsCard.route(from, to)
            }
            return null
        }

    companion object {
        /** Tools whose result is drawn as a card (map, recipe, route, stocks, products, flights, and a chart from its data). */
        val CARD_TOOLS = setOf("show_map", "show_recipe", "plan_trip", "compare_stocks", "search_products", "search_flights", "generate_chart")
    }
}

/** A choice the agent offers for a clarifying question. */
data class QuestionOption(val label: String, val description: String?)

/** One clarifying question from `ask_user`. */
data class Question(
    val question: String,
    val header: String?,
    val multiSelect: Boolean,
    val options: List<QuestionOption>,
)

/** What an `ask_user` call asked: the run stops after it, and the next message answers. */
data class Questions(val questions: List<Question>, val note: String?) {
    companion object {
        /** Parses the tool's JSON result. `null` when it holds no question. */
        fun parse(output: String): Questions? {
            val value = parseJson(output) ?: return null
            val items = value["questions"].arr ?: return null
            val questions = items.mapNotNull { item ->
                val text = item["question"].str?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
                val options = item["options"].arr?.mapNotNull { o ->
                    val label = o["label"].str?.trim()?.takeIf { it.isNotEmpty() } ?: return@mapNotNull null
                    QuestionOption(label, o["description"].str)
                }.orEmpty()
                Question(text, item["header"].str, item["multi_select"].bool ?: false, options)
            }
            if (questions.isEmpty()) return null
            return Questions(questions, value["note"].str)
        }
    }
}

/** A tool call waiting for the user's say-so (`approval.required`), and how it was decided. */
data class Approval(
    val id: String,
    val callId: String?,
    val tool: String,
    val input: JsonElement,
    val rule: String,
    val reason: String,
    val scope: String?,
    val decision: String? = null,
    val by: String? = null,
) {
    val isPending: Boolean get() = decision == null

    /** What "always" would remember: the host, the packages. */
    val alwaysLabel: String get() = if (!scope.isNullOrEmpty()) "Always for $scope" else "Always"
}

data class PlanStep(val id: String, val title: String)

/**
 * The steps the agent proposed (`plan`), and whether it waits for them to be agreed: before it
 * starts, or mid-run when it chose to plan with `propose_plan` (`origin` "tool").
 */
data class Plan(
    val steps: List<PlanStep>,
    val editable: Boolean,
    val awaiting: Boolean,
    val by: String? = null,
    val origin: String? = null,
) {
    /** Cedar chose to make this plan, partway through its work. */
    val isProposed: Boolean get() = origin == "tool"

    companion object {
        fun parseSteps(value: JsonElement?): List<PlanStep> {
            val items = value.arr ?: return emptyList()
            return items.mapIndexedNotNull { offset, item ->
                item.str?.let { return@mapIndexedNotNull PlanStep((offset + 1).toString(), it) }
                val title = item["title"].str ?: return@mapIndexedNotNull null
                val id = item["id"].str ?: item["id"].int?.toString() ?: (offset + 1).toString()
                PlanStep(id, title)
            }
        }
    }
}

/** What the agent produced during a turn, in order. */
sealed interface TranscriptItem {
    val id: String

    data class Text(val seq: Int, val text: String) : TranscriptItem {
        override val id get() = "text-$seq"
    }

    data class Reasoning(val seq: Int, val text: String, val durationMs: Int?) : TranscriptItem {
        override val id get() = "reasoning-$seq"
    }

    data class PlanItem(val seq: Int, val plan: Plan) : TranscriptItem {
        override val id get() = "plan-$seq"
    }

    data class ApprovalItem(val approval: Approval) : TranscriptItem {
        override val id get() = "approval-${approval.id}"
    }

    data class Tool(val call: ToolCall) : TranscriptItem {
        override val id get() = "tool-${call.id}"
    }
}

/**
 * One turn of a conversation: the user's message and the run that answers it, rebuilt from the run's
 * event log. Immutable: [apply] returns the next state.
 */
data class TurnTranscript(
    val turn: Turn,
    val items: List<TranscriptItem> = emptyList(),
    val usage: Usage = Usage(),
    /** The last event applied; reconnecting resumes after it. */
    val lastSeq: Int = 0,
    val cancelReason: String? = null,
    /** What `ask_user` asked, once its result arrived. */
    val questions: Questions? = null,
) {
    val id: String get() = turn.id
    val prompt: String get() = turn.prompt
    val isActive: Boolean get() = turn.isActive

    /** Applies one event. Events at or before `lastSeq` are ignored, so replays are harmless. */
    fun apply(event: RunEvent): TurnTranscript {
        if (event.seq <= lastSeq) return this
        val next = copy(lastSeq = event.seq)
        val data = event.data
        return when (event.type) {
            "run.started" -> next.copy(turn = turn.copy(status = TurnStatus.RUNNING), usage = Usage())
            "assistant.reasoning" -> {
                val text = data["text"].str
                if (text.isNullOrEmpty()) next else next.copy(items = items + TranscriptItem.Reasoning(event.seq, text, data["duration_ms"].int))
            }
            "assistant.text" -> {
                val text = data["text"].str
                if (text.isNullOrEmpty()) next else next.copy(items = items + TranscriptItem.Text(event.seq, text))
            }
            "plan" -> {
                val plan = Plan(
                    Plan.parseSteps(data["steps"]),
                    data["editable"].bool ?: false,
                    data["awaiting"].bool ?: false,
                    origin = data["origin"].str,
                )
                next.copy(items = items + TranscriptItem.PlanItem(event.seq, plan))
            }
            "plan.updated" -> {
                val index = items.indexOfLast { it is TranscriptItem.PlanItem }
                if (index < 0) return next
                val item = items[index] as TranscriptItem.PlanItem
                val steps = Plan.parseSteps(data["steps"])
                val plan = item.plan.copy(
                    steps = steps.ifEmpty { item.plan.steps },
                    awaiting = false,
                    by = data["by"].str,
                )
                next.copy(items = items.replaced(index, item.copy(plan = plan)))
            }
            "approval.required" -> {
                val approval = Approval(
                    id = data["id"].str ?: "seq-${event.seq}",
                    callId = data["call_id"].str,
                    tool = data["tool"].str ?: "tool",
                    input = data["input"] ?: JsonNull,
                    rule = data["rule"].str ?: "",
                    reason = data["reason"].str ?: "",
                    scope = data["scope"].str,
                )
                next.copy(items = items + TranscriptItem.ApprovalItem(approval))
            }
            "approval.resolved" -> {
                val id = data["id"].str ?: return next
                val index = items.indexOfLast { it.id == "approval-$id" }
                if (index < 0) return next
                val item = items[index] as? TranscriptItem.ApprovalItem ?: return next
                val approval = item.approval.copy(decision = data["decision"].str ?: "timeout", by = data["by"].str)
                next.copy(items = items.replaced(index, TranscriptItem.ApprovalItem(approval)))
            }
            "tool.call" -> {
                val call = ToolCall(
                    id = data["id"].str ?: "seq-${event.seq}",
                    name = data["name"].str ?: "tool",
                    input = data["input"] ?: JsonNull,
                )
                next.copy(items = items + TranscriptItem.Tool(call))
            }
            "tool.result" -> {
                val id = data["id"].str ?: return next
                val index = items.indexOfLast { it.id == "tool-$id" }
                if (index < 0) return next
                val item = items[index] as? TranscriptItem.Tool ?: return next
                val output = data["output"].str ?: ""
                val isError = data["is_error"].bool ?: false
                var call = item.call.copy(output = output, isError = isError, durationMs = data["duration_ms"].int)
                if (call.isCard && !isError) {
                    call = call.copy(card = Card.parse(call.name, output, call.input))
                } else if (!isError) {
                    call = call.copy(charts = ChartCard.inOutput(output).charts)
                }
                data["sources"].arr?.let { sources ->
                    call = call.copy(sources = sources.mapNotNull { s ->
                        val n = s["n"].int ?: return@mapNotNull null
                        val url = s["url"].str ?: return@mapNotNull null
                        Source(n, url, s["title"].str)
                    })
                }
                var updated = next.copy(items = items.replaced(index, TranscriptItem.Tool(call)))
                if (call.name == "ask_user" && !isError) updated = updated.copy(questions = Questions.parse(output))
                updated
            }
            "usage" -> {
                val cost = data["cost_usd"].dbl
                next.copy(usage = usage.copy(
                    inputTokens = usage.inputTokens + (data["input_tokens"].int ?: 0),
                    outputTokens = usage.outputTokens + (data["output_tokens"].int ?: 0),
                    costUSD = if (cost != null) (usage.costUSD ?: 0.0) + cost else usage.costUSD,
                ))
            }
            "run.completed" -> next.copy(turn = turn.copy(
                status = TurnStatus.COMPLETED,
                finalText = data["final_text"].str,
                stopReason = data["stop_reason"].str,
            ))
            "run.failed" -> next.copy(turn = turn.copy(status = TurnStatus.FAILED, error = data["error"].str))
            "run.cancelled" -> next.copy(turn = turn.copy(status = TurnStatus.CANCELLED), cancelReason = data["reason"].str)
            else -> next
        }
    }

    /** The agent's answer: the final text, or failing that the last text it wrote. */
    val answer: String
        get() {
            turn.finalText?.takeIf { it.isNotEmpty() }?.let { return it }
            return items.lastOrNull { it is TranscriptItem.Text }?.let { (it as TranscriptItem.Text).text } ?: ""
        }

    /** The approval the run is waiting for, if any. */
    val pendingApproval: Approval?
        get() = items.asReversed().firstNotNullOfOrNull { (it as? TranscriptItem.ApprovalItem)?.approval?.takeIf(Approval::isPending) }

    /** The plan the run is waiting to have agreed, if any. */
    val awaitingPlan: Plan?
        get() = items.asReversed().firstNotNullOfOrNull { (it as? TranscriptItem.PlanItem)?.plan?.takeIf(Plan::awaiting) }

    /** The images the turn's finished tool calls produced, each once, in order: workspace paths. */
    val images: List<String>
        // A chart drawn as a card isn't shown again as a picture.
        get() = produced(WorkspaceImages::isImage) { it.name == "generate_chart" && it.card != null }

    /** The PDFs the turn's finished tool calls produced, each once, in order: workspace paths. */
    val documents: List<String>
        get() = produced(WorkspaceImages::isPdf) { false }

    private fun produced(include: (String) -> Boolean, skip: (ToolCall) -> Boolean): List<String> {
        val seen = LinkedHashSet<String>()
        for (item in items) {
            val call = (item as? TranscriptItem.Tool)?.call ?: continue
            if (!call.isFinished || call.isError || skip(call)) continue
            seen += WorkspaceImages.filesProducedBy(call).filter(include)
        }
        return seen.take(WorkspaceImages.MAX_SHOWN)
    }
}

private fun <T> List<T>.replaced(index: Int, item: T): List<T> = toMutableList().also { it[index] = item }

/** Which workspace files a tool call produced that are images or PDFs; the same rule as the web UI's `image-view.js`. */
object WorkspaceImages {
    const val MAX_SHOWN = 12
    val EXTENSIONS = setOf("png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg")

    /** Tools whose `path` input is the file they write. */
    val FILE_TOOLS = setOf("write_file", "generate_chart", "download_file")
    const val CHANGED_HEADING = "\n[files created or modified]\n"

    /** The images [call] produced. */
    fun producedBy(call: ToolCall): List<String> = filesProducedBy(call).filter(::isImage)

    /** Every workspace file [call] wrote or changed. */
    fun filesProducedBy(call: ToolCall): List<String> {
        val paths = mutableListOf<String>()
        if (call.name in FILE_TOOLS) {
            call.input["path"].str?.let(paths::add)
        } else if ((call.name == "bash" || call.name == "run_python") && call.output != null) {
            val at = call.output.indexOf(CHANGED_HEADING)
            if (at >= 0) {
                for (line in call.output.substring(at + CHANGED_HEADING.length).split("\n")) {
                    if (!line.startsWith("- ")) break
                    paths += line.drop(2)
                }
            }
        }
        return paths.mapNotNull(::workspacePath)
    }

    /** A path relative to the workspace, or null if `path` points elsewhere. */
    fun workspacePath(path: String): String? {
        var p = path.trim()
        if (p.startsWith("/workspace/")) p = p.removePrefix("/workspace/") else if (p.startsWith("./")) p = p.drop(2)
        if (p.isEmpty() || p.startsWith("/")) return null
        if (p.split("/").any { it.isEmpty() || it == "." || it == ".." }) return null
        return p
    }

    fun isImage(path: String): Boolean = extension(path) in EXTENSIONS

    fun isPdf(path: String): Boolean = extension(path) == "pdf"

    /** What follows the path's last dot, lowercased, or null when it has none. */
    fun extension(path: String): String? {
        val dot = path.lastIndexOf('.')
        if (dot < 0) return null
        return path.substring(dot + 1).lowercase()
    }
}

/** How the app shows a workspace file: an image, a PDF, or null for files it leaves to the web UI (SVG among them, which Android doesn't decode). */
enum class WorkspaceFileKind {
    IMAGE, PDF;

    companion object {
        fun of(path: String): WorkspaceFileKind? = when {
            WorkspaceImages.isPdf(path) -> PDF
            WorkspaceImages.isImage(path) && WorkspaceImages.extension(path) != "svg" -> IMAGE
            else -> null
        }
    }
}

/**
 * The text that answers clarifying questions, as the web UI composes it (`clarify-view.js`): each
 * question, then what was chosen and written, so the model sees both.
 */
object ClarifyAnswer {
    data class Answer(val chosen: List<String> = emptyList(), val own: String = "")

    /**
     * The answer's text; `other` words an answer of one's own next to the options, and
     * `noPreference` a question left open, in the user's language (English by default).
     */
    fun text(
        questions: Questions,
        answers: List<Answer>,
        other: (String) -> String = { "Other: $it" },
        noPreference: String = "No preference",
    ): String =
        questions.questions.mapIndexed { index, q ->
            val answer = answers.getOrNull(index) ?: Answer()
            val parts = answer.chosen.toMutableList()
            val own = answer.own.trim()
            if (own.isNotEmpty()) parts += if (answer.chosen.isEmpty() && q.options.isEmpty()) own else other(own)
            val text = if (parts.isEmpty()) noPreference else parts.joinToString(", ")
            if (questions.questions.size == 1) "${q.question}\n$text" else "${index + 1}. ${q.question}\n$text"
        }.joinToString("\n\n")
}

/** An empty JSON object, for tool calls without input. */
val EmptyJson: JsonElement = JsonObject(emptyMap())
