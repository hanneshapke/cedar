import XCTest
@testable import Cedar

final class APITests: XCTestCase {
    func testDecodesConversationsAndTurns() throws {
        let json = """
        {"conversation":{"id":"c1","title":"Trip","model":"m","reasoning":null,"schedule_id":null,"tools":["web_search"],
          "unread":true,"needs_answer":true,"created_at":"2026-10-12T03:36:00.5Z","updated_at":"2026-10-12T03:40:00Z"},
         "turns":[{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"go","status":"completed",
                   "final_text":"Which city?","error":null,"stop_reason":"ask_user","finished_at":"2026-10-12T03:40:00Z",
                   "seen":false,"attachments":[{"name":"a.pdf","media_type":"application/pdf","size":12}],
                   "created_at":"2026-10-12T03:36:00Z"},
                  {"id":"t2","conversation_id":"c1","seq":2,"run_id":"r2","prompt":"Berlin","status":"running",
                   "final_text":null,"error":null,"stop_reason":null,"finished_at":null,"seen":true,"attachments":[],
                   "created_at":"2026-10-12T03:41:00Z"}]}
        """
        let detail = try JSONDecoder.cedar.decode(ConversationDetail.self, from: Data(json.utf8))
        XCTAssertEqual(detail.conversation.displayTitle, "Trip")
        XCTAssertTrue(detail.conversation.unread && detail.conversation.needsAnswer)
        XCTAssertEqual(detail.conversation.tools, ["web_search"])
        XCTAssertEqual(detail.conversation.createdAt.timeIntervalSince1970, 1_791_776_160.5, accuracy: 0.001)
        XCTAssertEqual(detail.turns.count, 2)
        XCTAssertTrue(detail.turns[0].needsAnswer)
        XCTAssertFalse(detail.turns[0].isActive)
        XCTAssertEqual(detail.turns[0].attachments.first?.mediaType, "application/pdf")
        XCTAssertTrue(detail.turns[1].isActive)
        XCTAssertFalse(detail.turns[1].needsAnswer)
        XCTAssertEqual(Conversation(id: "x", unread: false, needsAnswer: false, createdAt: Date(), updatedAt: Date()).displayTitle, "New chat")
    }

    func testDecodesInboxNoticesAndModels() throws {
        let inbox = try JSONDecoder.cedar.decode(Inbox.self, from: Data("""
        {"items":[{"turn_id":"t1","conversation_id":"c1","title":"Trip","schedule_id":null,"status":"failed",
                   "stop_reason":null,"needs_answer":false,"finished_at":"2026-10-12T03:40:00Z"}],"unread":1}
        """.utf8))
        XCTAssertEqual(inbox.unread, 1)
        XCTAssertEqual(inbox.items.first?.status, "failed")

        let notice = try JSONDecoder.cedar.decode(Notice.self, from: Data("""
        {"type":"turn.finished","turn_id":"t1","conversation_id":"c1","run_id":"r1","status":"completed",
         "stop_reason":"ask_user","error":null,"needs_answer":true,"finished_at":"2026-10-12T03:40:00Z","title":"Trip","schedule_id":null}
        """.utf8))
        XCTAssertEqual(notice.conversationId, "c1")
        XCTAssertTrue(notice.needsAnswer)

        let models = try JSONDecoder.cedar.decode(ModelList.self, from: Data("""
        {"models":[{"id":"deepseek","label":"DeepSeek V4","model":"deepseek-ai/DeepSeek-V4-Flash","reasoning_levels":["low","high"],
                    "default_reasoning":"low","multimodal":false,"is_default":true,"price":{"input_usd_per_m":1,"output_usd_per_m":2}},
                   {"id":"bare"}]}
        """.utf8))
        XCTAssertEqual(models.models.map(\.id), ["deepseek", "bare"])
        XCTAssertEqual(models.models[0].reasoningLevels, ["low", "high"])
        XCTAssertTrue(models.models[0].isDefault)
        XCTAssertEqual(models.models[1].label, "bare")
    }

    func testDecodesSchedulesAndRuns() throws {
        let list = try JSONDecoder.cedar.decode(ScheduleList.self, from: Data("""
        {"schedules":[{"id":"s1","title":"News","prompt":"Summarize the news","model":null,"reasoning":null,
          "source_conversation_id":null,"tools":null,"notify":null,"frequency":"weekly","weekday":0,"time_of_day":"07:30",
          "timezone":"Mars/Olympus","enabled":true,"next_run_at":"2026-10-26T06:30:00Z","last_run_at":"2026-10-19T06:30:00Z",
          "created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-01T00:00:00Z",
          "last_run":{"id":"r1","schedule_id":"s1","conversation_id":"c1","turn_id":"t1","due_at":"2026-10-19T06:30:00Z",
            "started_at":"2026-10-19T06:30:05Z","manual":false,"error":null,
            "turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"x","prompt":"Summarize the news","status":"completed",
                    "final_text":"Done","error":null,"stop_reason":null,"finished_at":"2026-10-19T06:31:00Z","seen":false,
                    "attachments":[],"created_at":"2026-10-19T06:30:05Z"}}},
         {"id":"s2","title":"Paused","prompt":"p","frequency":"daily","weekday":null,"time_of_day":"09:00",
          "timezone":"UTC","enabled":false,"next_run_at":"2026-10-23T09:00:00Z","last_run_at":null,"last_run":null}]}
        """.utf8))
        XCTAssertEqual(list.schedules.count, 2)
        let news = list.schedules[0]
        XCTAssertEqual(news.cadence, "Mondays at 07:30 (Mars/Olympus)")
        XCTAssertEqual(news.lastRun?.outcome, .completed)
        XCTAssertEqual(news.lastRun?.unread, true)
        XCTAssertFalse(list.schedules[1].enabled)
        XCTAssertNil(list.schedules[1].lastRun)

        let detail = try JSONDecoder.cedar.decode(ScheduleDetail.self, from: Data("""
        {"schedule":{"id":"s1","title":"News","prompt":"p","frequency":"weekdays","weekday":null,"time_of_day":"07:30",
                     "timezone":"Mars/Olympus","enabled":true,"next_run_at":"2026-10-26T06:30:00Z"},
         "runs":[{"id":"r2","schedule_id":"s1","conversation_id":null,"turn_id":null,"due_at":"2026-10-20T06:30:00Z",
                  "started_at":"2026-10-20T07:30:00Z","manual":true,"error":"cedar is busy","turn":null}]}
        """.utf8))
        XCTAssertEqual(detail.schedule.cadence, "Weekdays at 07:30 (Mars/Olympus)")
        XCTAssertEqual(detail.runs.first?.outcome, .notStarted)
        XCTAssertEqual(detail.runs.first?.unread, false)
        XCTAssertNil(detail.runs.first?.conversationId)
    }

    func testParsesDatesWithAnyPrecision() {
        XCTAssertNotNil(CedarDate.parse("2026-10-12T03:36:00Z"))
        XCTAssertNotNil(CedarDate.parse("2026-10-12T03:36:00.1Z"))
        XCTAssertNotNil(CedarDate.parse("2026-10-12T03:36:00.123456789+00:00"))
        XCTAssertNil(CedarDate.parse("yesterday"))
    }

    func testErrorEnvelope() {
        let body = Data(#"{"error":{"code":"conflict","message":"wait for the current response to finish, or stop it"}}"#.utf8)
        XCTAssertEqual(ChatClient.error(status: 409, body: body),
                       .api(status: 409, code: "conflict", message: "wait for the current response to finish, or stop it"))
        XCTAssertEqual(ChatClient.error(status: 401, body: Data()), .unauthorized)
    }

    func testTLSErrorOnExplicitPortHintsAtTailscaleServe() {
        let tls = URLError(.secureConnectionFailed)
        guard case .unreachable(let detail) = ChatClient.unreachable(tls, baseURL: URL(string: "https://spark.ts.net:3000")!) else {
            return XCTFail("expected .unreachable")
        }
        XCTAssertTrue(detail.contains("Port 3000 may serve plain HTTP"))
        XCTAssertEqual(ChatClient.unreachable(tls, baseURL: URL(string: "https://spark.ts.net")!),
                       .unreachable(tls.localizedDescription))
        let offline = URLError(.notConnectedToInternet)
        XCTAssertEqual(ChatClient.unreachable(offline, baseURL: URL(string: "https://spark.ts.net:3000")!),
                       .unreachable(offline.localizedDescription))
    }

    func testWebURLAndFileRequests() throws {
        let client = ChatClient(baseURL: URL(string: "http://chat.local:3000/")!, accessToken: "tok")
        XCTAssertEqual(client.webURL(conversationId: "c1").absoluteString, "http://chat.local:3000/#/c/c1")
        let request = client.fileRequest(conversationId: "c1", path: "out/my chart.png")
        XCTAssertEqual(request.url?.absoluteString, "http://chat.local:3000/api/conversations/c1/files/out/my%20chart.png")
        XCTAssertEqual(request.value(forHTTPHeaderField: "Authorization"), "Bearer tok")
        let open = try ChatClient(settings: ServerSettings(serverURL: "http://x", accessToken: "", model: "", reasoning: ""))
        XCTAssertNil(open.fileRequest(conversationId: "c", path: "a").value(forHTTPHeaderField: "Authorization"))
        XCTAssertThrowsError(try ChatClient(settings: ServerSettings(serverURL: "ftp://x", accessToken: "", model: "", reasoning: "")))
    }
}
