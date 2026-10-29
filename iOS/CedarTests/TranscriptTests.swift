import XCTest
@testable import Cedar

final class TranscriptTests: XCTestCase {
    private func turn(_ id: String = "t1", status: TurnStatus = .queued) -> Turn {
        Turn(id: id, conversationId: "c1", runId: "r1", prompt: "hi", status: status)
    }

    private func event(_ seq: Int, _ type: String, _ data: JSONValue) -> RunEvent {
        RunEvent(seq: seq, type: type, data: data)
    }

    func testAppliesEventsInOrderAndIgnoresReplays() {
        var transcript = TurnTranscript(turn: turn())
        let events: [(String, JSONValue)] = [
            ("run.started", .object([:])),
            ("assistant.reasoning", .object(["text": .string("Let me think."), "duration_ms": .number(1500), "done": .bool(true)])),
            ("tool.call", .object(["id": .string("t1"), "name": .string("bash"), "input": .object(["command": .string("ls")])])),
            ("tool.result", .object(["id": .string("t1"), "output": .string("a.txt"), "is_error": .bool(false), "duration_ms": .number(12)])),
            ("usage", .object(["input_tokens": .number(100), "output_tokens": .number(7), "cost_usd": .number(0.001)])),
            ("assistant.text", .object(["text": .string("Done.")])),
            ("run.completed", .object(["final_text": .string("Done."), "stop_reason": .string("end_turn")])),
        ]
        for (offset, (type, data)) in events.enumerated() {
            transcript.apply(event(offset + 1, type, data))
        }
        // A replayed event is ignored.
        transcript.apply(event(5, "usage", .object(["input_tokens": .number(100)])))

        XCTAssertEqual(transcript.turn.status, .completed)
        XCTAssertEqual(transcript.usage.inputTokens, 100)
        XCTAssertEqual(transcript.usage.outputTokens, 7)
        XCTAssertEqual(transcript.usage.costUSD ?? 0, 0.001, accuracy: 1e-9)
        XCTAssertEqual(transcript.items.count, 3)
        guard case .reasoning(_, let text, let ms) = transcript.items[0] else { return XCTFail("expected reasoning") }
        XCTAssertEqual(text, "Let me think.")
        XCTAssertEqual(ms, 1500)
        guard case .tool(let call) = transcript.items[1] else { return XCTFail("expected a tool call") }
        XCTAssertEqual(call.summary, "ls")
        XCTAssertEqual(call.output, "a.txt")
        XCTAssertEqual(call.durationMs, 12)
        XCTAssertEqual(transcript.answer, "Done.")
        XCTAssertTrue(transcript.images.isEmpty)
        XCTAssertNil(transcript.questions)
    }

    func testAskUserStopsTheRunWithQuestions() throws {
        var transcript = TurnTranscript(turn: turn())
        let output = """
        {"questions":[{"question":"Which city?","header":"City","multi_select":false,"options":[{"label":"Berlin","description":"the capital"},{"label":"Munich"}]},
                      {"question":"Anything else?","multi_select":true,"options":[]}],"note":"Pick one."}
        """
        transcript.apply(event(1, "tool.call", .object(["id": .string("q"), "name": .string("ask_user"), "input": .object([:])])))
        transcript.apply(event(2, "tool.result", .object(["id": .string("q"), "output": .string(output), "is_error": .bool(false)])))
        transcript.apply(event(3, "run.completed", .object(["final_text": .string("Which city?"), "stop_reason": .string("ask_user")])))
        XCTAssertTrue(transcript.turn.needsAnswer)
        let questions = try XCTUnwrap(transcript.questions)
        XCTAssertEqual(questions.questions.count, 2)
        XCTAssertEqual(questions.questions[0].header, "City")
        XCTAssertEqual(questions.questions[0].options.map(\.label), ["Berlin", "Munich"])
        XCTAssertEqual(questions.questions[0].options[0].description, "the capital")
        XCTAssertTrue(questions.questions[1].multiSelect)
        XCTAssertEqual(questions.note, "Pick one.")

        // The answer reads as the web UI composes it.
        let text = ClarifyAnswer.text(for: questions, answers: [.init(chosen: ["Berlin"], own: "or nearby"), .init()])
        XCTAssertEqual(text, "1. Which city?\nBerlin, Other: or nearby\n\n2. Anything else?\nNo preference")
        let one = Questions(questions: [questions.questions[1]], note: nil)
        XCTAssertEqual(ClarifyAnswer.text(for: one, answers: [.init(chosen: [], own: "no")]), "Anything else?\nno")
        XCTAssertNil(Questions.parse("{\"questions\":[]}"))
        XCTAssertNil(Questions.parse("not json"))
    }

    func testAPlanCedarProposesMidRunWaitsLikeAnyOther() throws {
        var transcript = TurnTranscript(turn: turn())
        transcript.apply(event(1, "assistant.text", .object(["turn": .number(1), "text": .string("Let me plan this.")])))
        transcript.apply(event(2, "plan", .object([
            "steps": .array([.object(["id": .string("1"), "title": .string("Search")]), .object(["id": .string("2"), "title": .string("Write")])]),
            "editable": .bool(true), "awaiting": .bool(true), "origin": .string("tool"),
        ])))
        XCTAssertEqual(transcript.awaitingPlan?.isProposed, true)
        transcript.apply(event(3, "plan.updated", .object(["steps": .array([.string("Search")]), "by": .string("timeout")])))
        guard case .plan(_, let plan) = transcript.items[1] else { return XCTFail("the plan comes after what Cedar said first") }
        XCTAssertTrue(plan.isProposed)
        XCTAssertFalse(plan.awaiting)
        XCTAssertEqual(plan.by, "timeout")

        var before = TurnTranscript(turn: turn())
        before.apply(event(1, "plan", .object(["steps": .array([.string("A"), .string("B")]), "awaiting": .bool(true)])))
        XCTAssertEqual(before.awaitingPlan?.isProposed, false)
    }

    func testPlansAndApprovalsWait() throws {
        var transcript = TurnTranscript(turn: turn())
        transcript.apply(event(1, "run.started", .object([:])))
        transcript.apply(event(2, "plan", .object([
            "steps": .array([.object(["id": .string("1"), "title": .string("Look")]), .object(["id": .string("2"), "title": .string("Leap")])]),
            "editable": .bool(true), "awaiting": .bool(true),
        ])))
        XCTAssertEqual(transcript.awaitingPlan?.steps.map(\.title), ["Look", "Leap"])
        transcript.apply(event(3, "plan.updated", .object(["steps": .array([.string("Look")]), "by": .string("user")])))
        XCTAssertNil(transcript.awaitingPlan)
        guard case .plan(_, let plan) = transcript.items[0] else { return XCTFail("expected a plan") }
        XCTAssertEqual(plan.steps.map(\.title), ["Look"])
        XCTAssertEqual(plan.by, "user")

        transcript.apply(event(4, "approval.required", .object([
            "id": .string("abc123"), "call_id": .string("t9"), "tool": .string("bash"),
            "input": .object(["command": .string("rm -rf build")]), "rule": .string("deletes"),
            "reason": .string("deletes files"), "scope": .null,
        ])))
        let pending = try XCTUnwrap(transcript.pendingApproval)
        XCTAssertEqual(pending.id, "abc123")
        XCTAssertEqual(pending.tool, "bash")
        XCTAssertEqual(pending.rule, "deletes")
        XCTAssertEqual(pending.alwaysLabel, "Always")
        transcript.apply(event(5, "approval.resolved", .object(["id": .string("abc123"), "decision": .string("deny"), "by": .string("user")])))
        XCTAssertNil(transcript.pendingApproval)
        guard case .approval(let decided) = transcript.items[1] else { return XCTFail("expected an approval") }
        XCTAssertEqual(decided.decision, "deny")
    }

    func testImagesComeFromFileToolsAndChangedFiles() {
        var transcript = TurnTranscript(turn: turn())
        let calls: [(String, JSONValue, String)] = [
            ("write_file", .object(["path": .string("/workspace/out/chart.png")]), "ok"),
            ("generate_chart", .object(["path": .string("./plot.svg")]), "ok"),
            ("write_file", .object(["path": .string("notes.md")]), "ok"),
            ("bash", .object(["command": .string("python draw.py")]), "drew\n[files created or modified]\n- a.png\n- data.csv\n- ../escape.png\n- b.jpeg\nnot a file"),
            ("write_file", .object(["path": .string("out/chart.png")]), "ok"),
        ]
        for (offset, (name, input, output)) in calls.enumerated() {
            let id = "c\(offset)"
            transcript.apply(event(offset * 2 + 1, "tool.call", .object(["id": .string(id), "name": .string(name), "input": input])))
            transcript.apply(event(offset * 2 + 2, "tool.result", .object(["id": .string(id), "output": .string(output), "is_error": .bool(false)])))
        }
        XCTAssertEqual(transcript.images, ["out/chart.png", "plot.svg", "a.png", "b.jpeg"])
        XCTAssertNil(WorkspaceImages.workspacePath("/etc/passwd"))
        XCTAssertNil(WorkspaceImages.workspacePath("a//b.png"))
        XCTAssertFalse(WorkspaceImages.isImage("photo"))
    }

    func testPDFsComeFromFileToolsAndChangedFiles() {
        var transcript = TurnTranscript(turn: turn())
        let calls: [(String, JSONValue, String, Bool)] = [
            ("write_file", .object(["path": .string("/workspace/report.pdf")]), "ok", false),
            ("bash", .object(["command": .string("make")]), "built\n[files created or modified]\n- out/Slides.PDF\n- out/a.png\n- report.pdf", false),
            ("write_file", .object(["path": .string("failed.pdf")]), "disk full", true),
        ]
        for (offset, (name, input, output, isError)) in calls.enumerated() {
            let id = "c\(offset)"
            transcript.apply(event(offset * 2 + 1, "tool.call", .object(["id": .string(id), "name": .string(name), "input": input])))
            transcript.apply(event(offset * 2 + 2, "tool.result", .object(["id": .string(id), "output": .string(output), "is_error": .bool(isError)])))
        }
        XCTAssertEqual(transcript.documents, ["report.pdf", "out/Slides.PDF"])
        XCTAssertEqual(transcript.images, ["out/a.png"])
    }

    func testTheViewerShowsImagesAndPDFs() {
        XCTAssertEqual(WorkspaceFileKind(path: "out/chart.PNG"), .image)
        XCTAssertEqual(WorkspaceFileKind(path: "docs/report.pdf"), .pdf)
        XCTAssertNil(WorkspaceFileKind(path: "plot.svg"))
        XCTAssertNil(WorkspaceFileKind(path: "notes.md"))
        XCTAssertNil(WorkspaceFileKind(path: "Makefile"))
    }

    func testMemoryStepsShowTheFact() {
        XCTAssertEqual(ToolCall(id: "1", name: "remember", input: .object(["text": .string("Prefers metric units")])).summary, "Prefers metric units")
        XCTAssertEqual(ToolCall(id: "2", name: "forget", input: .object(["id": .string("m1")])).summary, "")
    }

    func testCardTitles() {
        var call = ToolCall(id: "1", name: "compare_stocks", input: .object(["symbols": .array([.string("AAPL"), .string("MSFT")])]))
        XCTAssertEqual(call.summary, "AAPL, MSFT")
        XCTAssertTrue(call.isCard)
        call.output = #"{"symbols":[{"symbol":"AAPL"}],"news":[]}"#
        XCTAssertEqual(call.summary, "AAPL")
        let trip = ToolCall(id: "2", name: "plan_trip", input: .object(["from": .string("Home"), "to": .string("Work")]))
        XCTAssertEqual(trip.summary, "Home → Work")
        let map = ToolCall(id: "3", name: "show_map", input: .object([:]), output: #"{"title":"Cafés","markers":[]}"#)
        XCTAssertEqual(map.summary, "Cafés")
        let shop = ToolCall(id: "4", name: "search_products", input: .object(["query": .string("headphones")]))
        XCTAssertTrue(shop.isCard)
        XCTAssertEqual(shop.summary, "headphones")
        var flights = ToolCall(id: "5", name: "search_flights",
                               input: .object(["from": .array([.string("SFO"), .string("OAK")]), "to": .array([.string("MUC")]),
                                               "date": .string("2026-12-01")]))
        XCTAssertTrue(flights.isCard)
        XCTAssertEqual(flights.summary, "SFO/OAK → MUC")
        flights.output = #"{"kind":"flights","from":["MUC"],"to":["SFO"],"options":[]}"#
        XCTAssertEqual(flights.summary, "SFO/OAK → MUC", "the route asked for, also for the return flights")
        let bare = ToolCall(id: "6", name: "search_flights", input: .object([:]), output: #"{"kind":"flights","from":["SFO"],"to":["MUC"]}"#)
        XCTAssertEqual(bare.summary, "SFO → MUC")
    }

    func testAChartDrawnAsACardIsNotShownAgainAsAPicture() {
        var transcript = TurnTranscript(turn: turn())
        let input: JSONValue = .object(["path": .string("chart.png"), "kind": .string("bar"),
                                        "labels": .array([.string("a")]), "series": .array([.object(["values": .array([.number(1)])])])])
        transcript.apply(event(1, "tool.call", .object(["id": .string("c1"), "name": .string("generate_chart"), "input": input])))
        transcript.apply(event(2, "tool.result", .object(["id": .string("c1"), "output": .string("wrote chart.png"), "is_error": .bool(false)])))
        guard case .tool(let call)? = transcript.items.last else { return XCTFail("expected a tool call") }
        guard case .chart? = call.card else { return XCTFail("expected a chart card") }
        XCTAssertEqual(call.summary, "chart.png")
        XCTAssertEqual(transcript.images, [])
    }

    func testChartsAnyToolPrintsAreDrawn() {
        var transcript = TurnTranscript(turn: turn())
        let output = "Glucose report\n[chart] {\"kind\":\"bar\",\"labels\":[\"a\"],\"series\":[{\"values\":[1]}]}"
        transcript.apply(event(1, "tool.call", .object(["id": .string("g1"), "name": .string("tandem_glucose"), "input": .object([:])])))
        transcript.apply(event(2, "tool.result", .object(["id": .string("g1"), "output": .string(output), "is_error": .bool(false)])))
        guard case .tool(let call)? = transcript.items.last else { return XCTFail("expected a tool call") }
        XCTAssertNil(call.card)
        XCTAssertEqual(call.charts.count, 1)
    }
}
