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
