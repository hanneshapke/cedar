import XCTest
@testable import Cedar

final class TodayTests: XCTestCase {
    private func json(_ value: some Encodable) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? [String: Any])
    }

    func testDecodesTheTimeline() throws {
        let timeline = try JSONDecoder.cedar.decode(Timeline.self, from: Data("""
        {"day":"2026-11-27","timezone":"Europe/Zurich","today":true,"generated_at":"2026-11-27T09:42:00Z","needs_you":3,
         "earlier":[
          {"turn_id":"t1","conversation_id":"c1","title":"Morning brief","prompt":"Morning brief","state":"done","status":"completed",
           "stop_reason":"end_turn","summary":"Rain until noon.","error":null,"created_at":"2026-11-27T06:00:00Z",
           "finished_at":"2026-11-27T06:01:00Z","seen":true,"needs_answer":false,
           "agent":{"id":"a1","title":"Morning brief","trigger_kind":"time"},"waits":[]},
          {"turn_id":"t2","conversation_id":"c2","title":null,"prompt":"Check the strike","state":"failed","status":"failed",
           "summary":null,"error":"The search service didn't answer.","created_at":"2026-11-27T07:00:00Z","agent":null,"waits":[]}],
         "now":[
          {"turn_id":"t3","conversation_id":"c3","title":"Email the landlord","prompt":"Email the landlord","state":"waiting",
           "status":"running","created_at":"2026-11-27T09:30:00Z","needs_answer":false,
           "waits":[{"kind":"approval","key":"approval:a1","reason":"Send a message in your Gmail.","since":"2026-11-27T09:31:00Z"},
                    {"kind":"plan","key":"plan","reason":null,"since":"2026-11-27T09:31:00Z"}]},
          {"turn_id":"t4","conversation_id":"c4","title":"Cancel the gym","prompt":"Cancel the gym","state":"asking",
           "status":"completed","stop_reason":"ask_user","summary":"By letter or by email?","created_at":"2026-11-27T09:35:00Z",
           "needs_answer":true,"waits":[]},
          {"turn_id":"t5","conversation_id":"c5","title":"Summarise the lease","prompt":"Summarise the lease","state":"brand_new",
           "status":"running","created_at":"2026-11-27T09:40:00Z","waits":[]}],
         "outbox":[
          {"id":"h1","run_id":"r1","call_id":"x","tool":"gmail_send_message","input":{"to":"ana@example.com"},
           "reason":"Send message in your Gmail to ana@example.com.","why":"unattended","status":"pending","edited":false,
           "output":null,"created_at":"2026-11-27T09:20:00Z","decided_at":null,"turn_id":"t1","conversation_id":"c1",
           "title":"Morning brief","agent":{"id":"a1","title":"Morning brief"}},
          {"id":"h2","tool":"gmail_send_message","input":{},"reason":"","why":"timeout","status":"pending"}],
         "ahead":[
          {"id":"a1","title":"Morning brief","enabled":true,"trigger_kind":"time",
           "trigger":{"kind":"time","frequency":"weekdays","time":"07:00","timezone":"Europe/Zurich"},"watches":false,
           "next_run_at":"2026-11-30T06:00:00Z","last_run_at":null,"last_checked_at":null}],
         "watching":[
          {"id":"a2","title":"File receipts","enabled":true,"trigger_kind":"email","trigger":{"kind":"email","query":"receipt"},
           "watches":true,"next_run_at":null,"last_checked_at":"2026-11-27T09:37:00Z"},
          {"id":"a3","title":"From a newer server","enabled":true,"trigger_kind":"webhook","trigger":null}],
         "standing":[]}
        """.utf8))
        XCTAssertEqual(timeline.day, "2026-11-27")
        XCTAssertTrue(timeline.today)
        XCTAssertEqual(timeline.needsYou, 3)
        XCTAssertFalse(timeline.isEmpty)

        XCTAssertEqual(timeline.earlier.map(\.state), [.done, .failed])
        XCTAssertEqual(timeline.earlier[0].agent?.title, "Morning brief")
        XCTAssertEqual(timeline.earlier[0].line, "Rain until noon.")
        // No title: what was asked stands in; a failure's line is why it failed.
        XCTAssertEqual(timeline.earlier[1].displayTitle, "Check the strike")
        XCTAssertEqual(timeline.earlier[1].line, "The search service didn't answer.")

        let waiting = timeline.now[0]
        XCTAssertEqual(waiting.state, .waiting)
        XCTAssertTrue(waiting.state.needsYou)
        XCTAssertEqual(waiting.waits.map(\.approvalId), ["a1", nil])
        XCTAssertEqual(waiting.waits[0].reason, "Send a message in your Gmail.")
        XCTAssertTrue(waiting.waits[1].isPlan)
        XCTAssertEqual(timeline.now[1].state, .asking)
        XCTAssertTrue(timeline.now[1].needsAnswer)
        // A state from a newer server doesn't fail the river.
        XCTAssertEqual(timeline.now[2].state, .done)
        XCTAssertFalse(timeline.now[2].state.needsYou)

        XCTAssertEqual(timeline.outbox.map(\.id), ["h1", "h2"])
        XCTAssertEqual(timeline.outbox[0].turnId, "t1")
        XCTAssertEqual(timeline.outbox[0].item.origin, "Morning brief")
        XCTAssertTrue(timeline.outbox[0].item.isOpen)
        // A run this chat doesn't know: no turn, no origin.
        XCTAssertNil(timeline.outbox[1].turnId)
        XCTAssertNil(timeline.outbox[1].item.origin)

        XCTAssertEqual(timeline.ahead[0].trigger, .time(frequency: "weekdays", weekday: nil, time: "07:00", timezone: "Europe/Zurich"))
        XCTAssertFalse(timeline.ahead[0].watches)
        XCTAssertNotNil(timeline.ahead[0].nextRunAt)
        XCTAssertTrue(timeline.watching[0].watches)
        XCTAssertEqual(timeline.watching[0].trigger.kind, "email")
        // No trigger, no `watches`: its kind says both.
        XCTAssertEqual(timeline.watching[1].trigger, .unknown(kind: "webhook"))
        XCTAssertTrue(timeline.watching[1].watches)

        // A server that sends next to nothing still reads.
        let bare = try JSONDecoder.cedar.decode(Timeline.self, from: Data("{}".utf8))
        XCTAssertTrue(bare.isEmpty)
        XCTAssertEqual(bare.needsYou, 0)
    }

    func testDecodesAnOutcome() throws {
        let outcome = try JSONDecoder.cedar.decode(AskOutcome.self, from: Data("""
        {"turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"Plan Lisbon","status":"completed",
                 "final_text":"## Lisbon\\n\\nFriday 13 to Sunday 15 March works.","error":null,"stop_reason":"end_turn",
                 "finished_at":"2026-11-27T10:58:00Z","seen":true,"attachments":[],"rating":null,"created_at":"2026-11-27T10:31:00Z"},
         "conversation":{"id":"c1","title":"Plan Lisbon","private":false},
         "agent":null,"state":"done","summary":"Friday 13 to Sunday 15 March works.","later_turns":1,
         "steps":[{"id":"x1","name":"web_search","is_error":false},{"id":"x2","name":"bash","is_error":true}],
         "sources":[{"url":"https://visit.example/lisbon","path":null,"title":"Visit Lisbon"},
                    {"url":"javascript:alert(1)","path":null,"title":null},{"url":null,"path":"notes.md","title":null}],
         "followups":[{"call_id":"x3","output":"{\\"follow_up\\":{\\"title\\":\\"Fares\\",\\"prompt\\":\\"Check fares\\",\\"at\\":\\"2099-01-01T09:00:00Z\\"}}"},
                      {"call_id":"x4","output":"not a suggestion"}],
         "followup_decisions":[{"turn_id":"t1","call_id":"x3","status":"dismissed","agent_id":null,"decided_at":"2026-11-27T11:00:00Z"}],
         "memories":[{"id":"m1","text":"Prefers walking to taxis","kind":"preference"}],
         "records":[{"set":"trips","key":"lisbon","data":{"city":"Lisbon"}},{"set":"log","key":null,"data":{}}],
         "files":[{"path":"out/plan.md","size":120,"modified_at":"2026-11-27T10:57:00Z"}],
         "held":[{"id":"h0","tool":"gmail_send_message","input":{},"reason":"","why":"unattended","status":"sent"},
                 {"id":"h1","tool":"gmail_send_message","input":{},"reason":"","why":"unattended","status":"pending"}]}
        """.utf8))
        XCTAssertEqual(outcome.state, .done)
        XCTAssertEqual(outcome.turn.prompt, "Plan Lisbon")
        XCTAssertEqual(outcome.headline, "Friday 13 to Sunday 15 March works.")
        XCTAssertEqual(outcome.laterTurns, 1)
        XCTAssertFalse(outcome.conversation.isPrivate)
        XCTAssertEqual(outcome.steps.map(\.name), ["web_search", "bash"])
        XCTAssertEqual(outcome.steps.map(\.isError), [false, true])
        // Only http(s) addresses open; a source without a title is called by its address or path.
        XCTAssertEqual(outcome.sources.map(\.label), ["Visit Lisbon", "javascript:alert(1)", "notes.md"])
        XCTAssertEqual(outcome.sources.map { $0.link?.absoluteString }, ["https://visit.example/lisbon", nil, nil])
        XCTAssertEqual(outcome.followups[0].followUp?.title, "Fares")
        XCTAssertNil(outcome.followups[1].followUp)
        XCTAssertEqual(outcome.decision(for: outcome.followups[0])?.isDismissed, true)
        XCTAssertNil(outcome.decision(for: outcome.followups[1]))
        XCTAssertEqual(outcome.memories.map(\.text), ["Prefers walking to taxis"])
        XCTAssertEqual(outcome.records.map(\.set), ["trips", "log"])
        XCTAssertEqual(outcome.records.map(\.key), ["lisbon", ""])
        XCTAssertEqual(outcome.files.map(\.path), ["out/plan.md"])
        XCTAssertEqual(outcome.heldOpenFirst.map(\.id), ["h1", "h0"])

        // A failure's headline is why it failed; a server that leaves the parts out still reads.
        let failed = try JSONDecoder.cedar.decode(AskOutcome.self, from: Data("""
        {"turn":{"id":"t2","conversation_id":"c2","run_id":"r2","prompt":"Check the strike","status":"failed","error":"No answer."},
         "conversation":{"id":"c2","title":null},"state":"failed"}
        """.utf8))
        XCTAssertEqual(failed.headline, "No answer.")
        XCTAssertTrue(failed.steps.isEmpty && failed.files.isEmpty && failed.held.isEmpty && failed.followups.isEmpty)
        XCTAssertEqual(failed.laterTurns, 0)
    }

    func testAnAskRunsNowOrBecomesAnAgentThatFitsItsWhen() throws {
        let zone = "Europe/Zurich"
        let now = try XCTUnwrap(CedarDate.parse("2026-11-27T09:42:10Z"))
        var draft = AskDraft(now: now, timezone: zone)
        // Two hours on, on the hour; mornings at seven.
        XCTAssertEqual(AgentClock.at(draft.laterAt, timezone: zone), "2026-11-27T12:00")
        XCTAssertEqual(AgentClock.time(draft.time, timezone: zone), "07:00")
        XCTAssertNotNil(draft.problem(now: now), "nothing to ask yet")

        // Now: a message, off the record only when chosen, and no trigger.
        draft.text = "  Summarise the lease  "
        XCTAssertNil(draft.problem(now: now))
        XCTAssertNil(draft.trigger())
        var body = try json(draft.body(locale: "de-CH", plan: "auto"))
        XCTAssertEqual(body["content"] as? String, "Summarise the lease")
        XCTAssertEqual(body["locale"] as? String, "de-CH")
        XCTAssertEqual(body["plan"] as? String, "auto")
        XCTAssertEqual(body["timezone"] as? String, zone)
        XCTAssertNil(body["when"])
        XCTAssertNil(body["trigger"])
        XCTAssertNil(body["private"], "sent only for an ask off the record")
        draft.offTheRecord = true
        XCTAssertEqual(try json(draft.body())["private"] as? Bool, true)

        // Later: once, at the picked instant in this device's zone. Nothing of it is private.
        draft.when = .later
        XCTAssertEqual(draft.trigger(), .once(at: "2026-11-27T12:00", timezone: zone))
        body = try json(draft.body(locale: "de-CH", model: "m", plan: "auto"))
        XCTAssertEqual(body["when"] as? String, "later")
        XCTAssertEqual((body["trigger"] as? [String: Any])?["kind"] as? String, "once")
        XCTAssertEqual(Set(body.keys), ["content", "when", "trigger", "timezone"], "the server refuses the rest")
        draft.laterAt = now.addingTimeInterval(-60)
        XCTAssertNotNil(draft.problem(now: now), "a time that has passed")

        // Every…: the clock's frequencies; a weekday only for weekly.
        draft.when = .every
        XCTAssertNil(draft.problem(now: now))
        XCTAssertEqual(draft.trigger(), .time(frequency: "daily", weekday: nil, time: "07:00", timezone: zone))
        draft.frequency = .weekly
        draft.weekday = 6
        XCTAssertEqual(draft.trigger(), .time(frequency: "weekly", weekday: 6, time: "07:00", timezone: zone))
        draft.frequency = .weekdays
        XCTAssertEqual(draft.trigger(), .time(frequency: "weekdays", weekday: nil, time: "07:00", timezone: zone))
        XCTAssertEqual(try json(draft.body())["when"] as? String, "every")

        // When something changes: mail needs its search; the calendar and the device don't.
        draft.when = .watch
        XCTAssertNotNil(draft.problem(now: now), "which mail?")
        draft.query = " from:bank "
        XCTAssertNil(draft.problem(now: now))
        XCTAssertEqual(draft.trigger(checkMinutes: 10), .email(query: "from:bank", everyMinutes: 10))
        draft.watch = .calendar
        XCTAssertEqual(draft.trigger(), .calendar(calendarId: nil, filter: nil, minutesBefore: 30, newEvents: false, everyMinutes: 5))
        draft.watch = .device
        XCTAssertEqual(draft.trigger(), .device)
        XCTAssertNil(draft.problem(now: now))
        body = try json(draft.body())
        XCTAssertEqual(body["when"] as? String, "watch")
        XCTAssertEqual((body["trigger"] as? [String: Any])?["kind"] as? String, "device")
    }

    func testDecodesWhatAnAskAnswers() throws {
        let now = try JSONDecoder.cedar.decode(AskAnswer.self, from: Data("""
        {"when":"now",
         "conversation":{"id":"c1","title":"Secret","model":null,"reasoning":null,"schedule_id":null,"tools":null,"unread":false,
                         "needs_answer":false,"starred_at":null,"private":true,
                         "created_at":"2026-11-27T10:00:00Z","updated_at":"2026-11-27T10:00:00Z"},
         "turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"Secret","status":"queued",
                 "created_at":"2026-11-27T10:00:00Z"}}
        """.utf8))
        XCTAssertEqual(now.conversation?.id, "c1")
        XCTAssertEqual(now.conversation?.isPrivate, true)
        XCTAssertEqual(now.turn?.status, .queued)
        XCTAssertNil(now.agent)

        let later = try JSONDecoder.cedar.decode(AskAnswer.self, from: Data("""
        {"when":"later","agent":{"id":"a1","title":"Call the dentist","prompt":"Call the dentist","enabled":true,
                                 "trigger":{"kind":"once","at":"2026-12-03T09:00","timezone":"UTC"},
                                 "next_run_at":"2026-12-03T09:00:00Z"}}
        """.utf8))
        XCTAssertEqual(later.when, "later")
        XCTAssertNil(later.conversation)
        XCTAssertEqual(later.agent?.trigger, .once(at: "2026-12-03T09:00", timezone: "UTC"))
    }
}
