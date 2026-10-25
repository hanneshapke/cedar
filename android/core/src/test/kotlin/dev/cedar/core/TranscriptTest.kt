package dev.cedar.core

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** JSON from text, for test payloads. */
fun j(text: String): JsonElement = parseJson(text) ?: error("bad JSON in test: $text")

fun event(seq: Int, type: String, data: String) = RunEvent(seq, type, j(data))

fun turn(id: String = "t1", status: TurnStatus = TurnStatus.QUEUED) =
    Turn(id = id, conversationId = "c1", runId = "r1", prompt = "hi", status = status, seq = 1)

/** A `tool.result` payload with `output` as a JSON string. */
fun result(id: String, output: String, isError: Boolean = false): String = buildJsonObject {
    put("id", id)
    put("output", JsonPrimitive(output))
    put("is_error", isError)
}.toString()

// Ported from iOS/CedarTests/TranscriptTests.swift.
class TranscriptTest {
    @Test
    fun appliesEventsInOrderAndIgnoresReplays() {
        var t = TurnTranscript(turn())
        val events = listOf(
            "run.started" to "{}",
            "assistant.reasoning" to """{"text":"Let me think.","duration_ms":1500,"done":true}""",
            "tool.call" to """{"id":"t1","name":"bash","input":{"command":"ls"}}""",
            "tool.result" to """{"id":"t1","output":"a.txt","is_error":false,"duration_ms":12}""",
            "usage" to """{"input_tokens":100,"output_tokens":7,"cost_usd":0.001}""",
            "assistant.text" to """{"text":"Done."}""",
            "run.completed" to """{"final_text":"Done.","stop_reason":"end_turn"}""",
        )
        events.forEachIndexed { offset, (type, data) -> t = t.apply(event(offset + 1, type, data)) }
        // A replayed event is ignored.
        t = t.apply(event(5, "usage", """{"input_tokens":100}"""))

        assertEquals(TurnStatus.COMPLETED, t.turn.status)
        assertEquals(100, t.usage.inputTokens)
        assertEquals(7, t.usage.outputTokens)
        assertEquals(0.001, t.usage.costUSD!!, 1e-9)
        assertEquals(3, t.items.size)
        val reasoning = assertIs<TranscriptItem.Reasoning>(t.items[0])
        assertEquals("Let me think.", reasoning.text)
        assertEquals(1500, reasoning.durationMs)
        val call = assertIs<TranscriptItem.Tool>(t.items[1]).call
        assertEquals("ls", call.summary)
        assertEquals("a.txt", call.output)
        assertEquals(12, call.durationMs)
        assertEquals("Done.", t.answer)
        assertTrue(t.images.isEmpty())
        assertNull(t.questions)
    }

    @Test
    fun askUserStopsTheRunWithQuestions() {
        var t = TurnTranscript(turn())
        val output = """{"questions":[{"question":"Which city?","header":"City","multi_select":false,"options":[{"label":"Berlin","description":"the capital"},{"label":"Munich"}]},
                      {"question":"Anything else?","multi_select":true,"options":[]}],"note":"Pick one."}"""
        t = t.apply(event(1, "tool.call", """{"id":"q","name":"ask_user","input":{}}"""))
        t = t.apply(event(2, "tool.result", result("q", output)))
        t = t.apply(event(3, "run.completed", """{"final_text":"Which city?","stop_reason":"ask_user"}"""))
        assertTrue(t.turn.needsAnswer)
        val questions = assertNotNull(t.questions)
        assertEquals(2, questions.questions.size)
        assertEquals("City", questions.questions[0].header)
        assertEquals(listOf("Berlin", "Munich"), questions.questions[0].options.map { it.label })
        assertEquals("the capital", questions.questions[0].options[0].description)
        assertTrue(questions.questions[1].multiSelect)
        assertEquals("Pick one.", questions.note)

        // The answer reads as the web UI composes it.
        val text = ClarifyAnswer.text(questions, listOf(ClarifyAnswer.Answer(listOf("Berlin"), "or nearby"), ClarifyAnswer.Answer()))
        assertEquals("1. Which city?\nBerlin, Other: or nearby\n\n2. Anything else?\nNo preference", text)
        val one = Questions(listOf(questions.questions[1]), null)
        assertEquals("Anything else?\nno", ClarifyAnswer.text(one, listOf(ClarifyAnswer.Answer(emptyList(), "no"))))
        assertNull(Questions.parse("""{"questions":[]}"""))
        assertNull(Questions.parse("not json"))
    }

    @Test
    fun plansAndApprovalsWait() {
        var t = TurnTranscript(turn())
        t = t.apply(event(1, "run.started", "{}"))
        t = t.apply(event(2, "plan", """{"steps":[{"id":"1","title":"Look"},{"id":"2","title":"Leap"}],"editable":true,"awaiting":true}"""))
        assertEquals(listOf("Look", "Leap"), t.awaitingPlan?.steps?.map { it.title })
        t = t.apply(event(3, "plan.updated", """{"steps":["Look"],"by":"user"}"""))
        assertNull(t.awaitingPlan)
        val plan = assertIs<TranscriptItem.PlanItem>(t.items[0]).plan
        assertEquals(listOf("Look"), plan.steps.map { it.title })
        assertEquals("user", plan.by)

        t = t.apply(event(4, "approval.required", """{"id":"abc123","call_id":"t9","tool":"bash","input":{"command":"rm -rf build"},"rule":"deletes","reason":"deletes files","scope":null}"""))
        val pending = assertNotNull(t.pendingApproval)
        assertEquals("abc123", pending.id)
        assertEquals("bash", pending.tool)
        assertEquals("deletes", pending.rule)
        assertEquals("Always", pending.alwaysLabel)
        t = t.apply(event(5, "approval.resolved", """{"id":"abc123","decision":"deny","by":"user"}"""))
        assertNull(t.pendingApproval)
        assertEquals("deny", assertIs<TranscriptItem.ApprovalItem>(t.items[1]).approval.decision)
    }

    @Test
    fun imagesComeFromFileToolsAndChangedFiles() {
        var t = TurnTranscript(turn())
        val calls = listOf(
            Triple("write_file", """{"path":"/workspace/out/chart.png"}""", "ok"),
            Triple("generate_chart", """{"path":"./plot.svg"}""", "ok"),
            Triple("write_file", """{"path":"notes.md"}""", "ok"),
            Triple("bash", """{"command":"python draw.py"}""", "drew\n[files created or modified]\n- a.png\n- data.csv\n- ../escape.png\n- b.jpeg\nnot a file"),
            Triple("write_file", """{"path":"out/chart.png"}""", "ok"),
        )
        calls.forEachIndexed { offset, (name, input, output) ->
            val id = "c$offset"
            t = t.apply(event(offset * 2 + 1, "tool.call", """{"id":"$id","name":"$name","input":$input}"""))
            t = t.apply(event(offset * 2 + 2, "tool.result", result(id, output)))
        }
        assertEquals(listOf("out/chart.png", "plot.svg", "a.png", "b.jpeg"), t.images)
        assertNull(WorkspaceImages.workspacePath("/etc/passwd"))
        assertNull(WorkspaceImages.workspacePath("a//b.png"))
        assertTrue(!WorkspaceImages.isImage("photo"))
    }

    @Test
    fun cardTitles() {
        var call = ToolCall("1", "compare_stocks", j("""{"symbols":["AAPL","MSFT"]}"""))
        assertEquals("AAPL, MSFT", call.summary)
        assertTrue(call.isCard)
        call = call.copy(output = """{"symbols":[{"symbol":"AAPL"}],"news":[]}""")
        assertEquals("AAPL", call.summary)
        val trip = ToolCall("2", "plan_trip", j("""{"from":"Home","to":"Work"}"""))
        assertEquals("Home → Work", trip.summary)
        val map = ToolCall("3", "show_map", j("{}"), output = """{"title":"Cafés","markers":[]}""")
        assertEquals("Cafés", map.summary)
        val shop = ToolCall("4", "search_products", j("""{"query":"headphones"}"""))
        assertTrue(shop.isCard)
        assertEquals("headphones", shop.summary)
    }

    @Test
    fun aChartDrawnAsACardIsNotShownAgainAsAPicture() {
        var t = TurnTranscript(turn())
        val input = """{"path":"chart.png","kind":"bar","labels":["a"],"series":[{"values":[1]}]}"""
        t = t.apply(event(1, "tool.call", """{"id":"c1","name":"generate_chart","input":$input}"""))
        t = t.apply(event(2, "tool.result", result("c1", "wrote chart.png")))
        val call = assertIs<TranscriptItem.Tool>(t.items.last()).call
        assertIs<Card.Chart>(call.card)
        assertEquals("chart.png", call.summary)
        assertEquals(emptyList(), t.images)
    }

    @Test
    fun chartsAnyToolPrintsAreDrawn() {
        var t = TurnTranscript(turn())
        val output = "Glucose report\n[chart] {\"kind\":\"bar\",\"labels\":[\"a\"],\"series\":[{\"values\":[1]}]}"
        t = t.apply(event(1, "tool.call", """{"id":"g1","name":"tandem_glucose","input":{}}"""))
        t = t.apply(event(2, "tool.result", result("g1", output)))
        val call = assertIs<TranscriptItem.Tool>(t.items.last()).call
        assertNull(call.card)
        assertEquals(1, call.charts.size)
    }
}
