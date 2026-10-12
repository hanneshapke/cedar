import XCTest
@testable import Cedar

final class ModelTests: XCTestCase {
    private func run(_ id: String, parent: String? = nil, minute: Int, status: RunStatus = .completed, prompt: String = "hi") -> Run {
        Run(id: id, status: status, prompt: prompt, model: "m", turns: 1, finalText: nil, stopReason: nil,
            error: nil, usage: .zero, parentId: parent,
            createdAt: Date(timeIntervalSince1970: TimeInterval(minute * 60)), finishedAt: nil)
    }

    func testDecodesRun() throws {
        let json = """
        {"id":"0190","status":"running","prompt":"p","system":"s","model":"m","max_turns":25,"turns":2,
         "final_text":null,"stop_reason":null,"error":null,"usage":{"input_tokens":10,"output_tokens":5},
         "parent_id":null,"workspace_id":"w","workspace_reused":false,
         "created_at":"2026-10-12T03:36:00.5Z","started_at":null,"finished_at":null}
        """
        let run = try JSONDecoder.cedar.decode(Run.self, from: Data(json.utf8))
        XCTAssertEqual(run.status, .running)
        XCTAssertEqual(run.usage, Usage(inputTokens: 10, outputTokens: 5))
        XCTAssertFalse(run.status.isTerminal)
        XCTAssertEqual(run.createdAt.timeIntervalSince1970, 1_791_776_160.5, accuracy: 0.001)
    }

    func testParsesDatesWithAnyPrecision() {
        XCTAssertNotNil(CedarDate.parse("2026-10-12T03:36:00Z"))
        XCTAssertNotNil(CedarDate.parse("2026-10-12T03:36:00.1Z"))
        XCTAssertNotNil(CedarDate.parse("2026-10-12T03:36:00.123456789+00:00"))
        XCTAssertNil(CedarDate.parse("yesterday"))
    }

    func testGroupsRunsIntoChains() {
        let runs = [
            run("c", parent: "b", minute: 3),
            run("x", minute: 2, prompt: "other"),
            run("b", parent: "a", minute: 1),
            run("a", minute: 0, prompt: "first\nsecond"),
            run("orphan", parent: "deleted", minute: 4),
        ]
        let conversations = Conversation.group(runs)
        XCTAssertEqual(conversations.map(\.id), ["orphan", "a", "x"])
        XCTAssertEqual(conversations[1].runs.map(\.id), ["a", "b", "c"])
        XCTAssertEqual(conversations[1].defaultTitle, "first")
    }

    func testExchangeAppliesEvents() {
        var exchange = Exchange(run: run("r", minute: 0, status: .queued))
        let events: [(String, JSONValue)] = [
            ("run.started", .object([:])),
            ("tool.call", .object(["id": .string("t1"), "name": .string("bash"), "input": .object(["command": .string("ls")])])),
            ("tool.result", .object(["id": .string("t1"), "output": .string("a.txt"), "is_error": .bool(false), "duration_ms": .number(12)])),
            ("usage", .object(["input_tokens": .number(100), "output_tokens": .number(7)])),
            ("assistant.text", .object(["text": .string("Done.")])),
            ("run.completed", .object(["final_text": .string("Done."), "stop_reason": .string("end_turn")])),
        ]
        for (offset, (type, data)) in events.enumerated() {
            exchange.apply(RunEvent(seq: offset + 1, type: type, data: data))
        }
        // A replayed event is ignored.
        exchange.apply(RunEvent(seq: 4, type: "usage", data: .object(["input_tokens": .number(100)])))

        XCTAssertEqual(exchange.run.status, .completed)
        XCTAssertEqual(exchange.usage, Usage(inputTokens: 100, outputTokens: 7))
        XCTAssertEqual(exchange.items.count, 2)
        guard case .tool(let call) = exchange.items[0] else { return XCTFail("expected a tool call") }
        XCTAssertEqual(call.summary, "ls")
        XCTAssertEqual(call.output, "a.txt")
        XCTAssertEqual(call.durationMs, 12)
        XCTAssertEqual(exchange.answer, "Done.")
    }

    func testComposeInlinesHistoryNewestKept() {
        XCTAssertEqual(PromptComposer.compose(history: [], message: "hello"), "hello")
        let long = String(repeating: "x", count: 500)
        let prompt = PromptComposer.compose(
            history: [(user: "old", assistant: long), (user: "new", assistant: "short")],
            message: "now", maxHistoryBytes: 200)
        XCTAssertFalse(prompt.contains("old"))
        XCTAssertTrue(prompt.contains("<user>\nnew\n</user>"))
        XCTAssertTrue(prompt.contains("1 earlier exchange(s) omitted"))
        XCTAssertTrue(prompt.hasSuffix("now"))
    }

    func testErrorEnvelope() {
        let body = Data(#"{"error":{"code":"conflict","message":"run is not active"}}"#.utf8)
        XCTAssertEqual(CedarClient.error(status: 409, body: body),
                       .api(status: 409, code: "conflict", message: "run is not active"))
        XCTAssertEqual(CedarClient.error(status: 401, body: Data()), .unauthorized)
    }
}
