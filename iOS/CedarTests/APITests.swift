import XCTest
@testable import Cedar

final class APITests: XCTestCase {
    func testDecodesMemoryAndTheConfigFlag() throws {
        // A server from before memory leaves the flag out.
        let old = try JSONDecoder.cedar.decode(ServerConfig.self, from: Data(#"{"auth_required":true,"admin_enabled":false}"#.utf8))
        XCTAssertFalse(old.memory)
        let new = try JSONDecoder.cedar.decode(ServerConfig.self, from: Data(#"{"auth_required":false,"admin_enabled":false,"memory":true}"#.utf8))
        XCTAssertTrue(new.memory)

        let list = try JSONDecoder.cedar.decode(MemoryList.self, from: Data("""
        {"supported":true,"limits":{"max_items":500,"max_chars":500,"max_profile_chars":1600},
         "profile":{"text":"Ana, in Zurich.","user_edited":false,"updated_at":"2026-11-06T08:00:00Z"},"items":[
          {"id":"m1","short_id":"m1","kind":"preference","text":"Prefers metric units","support":"I always want metric units",
           "pinned":false,"source":"user","recall_count":3,"run_id":"r1","conversation_id":"c1","title":"Units",
           "valid_from":"2026-10-29T08:00:00Z","valid_until":null,"archived_at":null,"superseded_by":null,
           "created_at":"2026-10-29T08:00:00Z","updated_at":"2026-10-29T08:00:00Z"},
          {"id":"m2","text":"Has a cat","run_id":null,"conversation_id":null,"title":null,
           "created_at":"2026-10-29T09:00:00Z","updated_at":"2026-10-29T09:00:00Z"}]}
        """.utf8))
        XCTAssertTrue(list.supported)
        XCTAssertEqual(list.limits?.maxItems, 500)
        XCTAssertEqual(list.profile?.text, "Ana, in Zurich.")
        XCTAssertEqual(list.profile?.userEdited, false)
        XCTAssertEqual(list.items.map(\.id), ["m1", "m2"])
        XCTAssertEqual(list.items[0].conversationId, "c1")
        XCTAssertEqual(list.items[0].title, "Units")
        XCTAssertEqual(list.items[0].kind, "preference")
        XCTAssertEqual(list.items[0].support, "I always want metric units")
        XCTAssertEqual(list.items[0].recallCount, 3)
        XCTAssertTrue(list.items[0].isActive)
        // A cedar from before kinds: a fact, not pinned, with no quote.
        XCTAssertNil(list.items[1].conversationId)
        XCTAssertEqual(list.items[1].kind, "fact")
        XCTAssertFalse(list.items[1].pinned)
        XCTAssertNil(list.items[1].support)
        XCTAssertNil(list.items[1].validFrom)
        let unsupported = try JSONDecoder.cedar.decode(MemoryList.self, from: Data(#"{"supported":false,"items":[],"limits":null}"#.utf8))
        XCTAssertFalse(unsupported.supported)
    }

    func testDecodesRecordsAndTheConfigFlag() throws {
        let old = try JSONDecoder.cedar.decode(ServerConfig.self, from: Data(#"{"auth_required":true,"admin_enabled":false,"memory":true}"#.utf8))
        XCTAssertFalse(old.records)
        let new = try JSONDecoder.cedar.decode(ServerConfig.self, from: Data(#"{"auth_required":false,"admin_enabled":false,"memory":true,"records":true}"#.utf8))
        XCTAssertTrue(new.records)

        let list = try JSONDecoder.cedar.decode(RecordsList.self, from: Data("""
        {"supported":true,"limits":{"max_sets":50,"max_items":10000,"max_record_chars":8000,"max_batch":500},"sets":[
          {"id":"s1","name":"running_log","description":"Weekly runs","key_fields":["date"],"health_data":false,"count":2,
           "columns":["date","km"],"created_at":"2026-10-29T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"},
          {"id":"s2","name":"weight","description":"","key_fields":[],"health_data":true,"count":0,"columns":[],
           "created_at":"2026-10-29T08:00:00Z","updated_at":"2026-10-29T08:00:00Z"}]}
        """.utf8))
        XCTAssertTrue(list.supported)
        XCTAssertEqual(list.limits?.maxSets, 50)
        XCTAssertEqual(list.sets.map(\.name), ["running_log", "weight"])
        XCTAssertEqual(list.sets[0].keyFields, ["date"])
        XCTAssertEqual(list.sets[0].columns, ["date", "km"])
        XCTAssertTrue(list.sets[1].healthData)
        let unsupported = try JSONDecoder.cedar.decode(RecordsList.self, from: Data(#"{"supported":false,"sets":[],"limits":null}"#.utf8))
        XCTAssertFalse(unsupported.supported)

        let detail = try JSONDecoder.cedar.decode(RecordSetDetail.self, from: Data("""
        {"set":{"id":"s1","name":"running_log","description":"Weekly runs","key_fields":["date"],"health_data":false,"count":2,
                "columns":["date","km"],"created_at":"2026-10-29T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"},
         "records":[{"id":"r1","key":"2026-10-26","data":{"date":"2026-10-26","km":31.2},"run_id":"run-1","conversation_id":"c1","title":"Week 43",
                     "created_at":"2026-10-26T08:00:00Z","updated_at":"2026-10-26T08:00:00Z"},
                    {"id":"r2","key":"2026-11-02","data":{"date":"2026-11-02","km":35,"note":null},"run_id":null,"conversation_id":null,"title":null,
                     "created_at":"2026-11-02T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"}]}
        """.utf8))
        XCTAssertEqual(detail.set.name, "running_log")
        XCTAssertEqual(detail.records.map(\.id), ["r1", "r2"])
        XCTAssertEqual(detail.records[0].conversationId, "c1")
        XCTAssertEqual(detail.records[0].data["km"], .number(31.2))
        XCTAssertNil(detail.records[1].conversationId)
        XCTAssertNil(detail.records[1].runId)
        XCTAssertEqual(detail.records[1].data["note"], .null)
    }

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

    func testDecodesStarredConversations() throws {
        let list = try JSONDecoder.cedar.decode(ConversationList.self, from: Data("""
        {"conversations":[{"id":"c2","title":"Morning digest · Fri 23 Oct","model":null,"reasoning":null,"schedule_id":"s1",
           "tools":null,"unread":false,"needs_answer":false,"starred_at":"2026-10-23T08:00:00Z","schedule_title":"Morning digest",
           "created_at":"2026-10-23T07:00:00Z","updated_at":"2026-10-23T07:01:00Z"},
          {"id":"c1","title":"Trip","model":null,"reasoning":null,"schedule_id":null,"tools":null,"unread":true,
           "needs_answer":false,"starred_at":"2026-10-22T08:00:00Z","schedule_title":null,
           "created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:40:00Z"}]}
        """.utf8))
        XCTAssertEqual(list.conversations.map(\.id), ["c2", "c1"])
        XCTAssertTrue(list.conversations.allSatisfy(\.isStarred))
        XCTAssertEqual(list.conversations[0].scheduleTitle, "Morning digest")
        XCTAssertNil(list.conversations[1].scheduleTitle)

        // The main list sends `starred_at` but no `schedule_title`; older servers send neither.
        let plain = try JSONDecoder.cedar.decode(Conversation.self, from: Data("""
        {"id":"c3","title":null,"model":null,"reasoning":null,"schedule_id":null,"tools":null,"unread":false,
         "needs_answer":false,"created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:40:00Z"}
        """.utf8))
        XCTAssertFalse(plain.isStarred)
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

    // MARK: Agents

    /// A value as the JSON it is sent as, to compare with what the server's contract says.
    private func sent<T: Encodable>(_ value: T) throws -> NSDictionary {
        try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? NSDictionary)
    }

    private func object(_ json: String) throws -> NSDictionary {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(json.utf8)) as? NSDictionary)
    }

    private func agent(_ json: String) throws -> Agent {
        try JSONDecoder.cedar.decode(Agent.self, from: Data(json.utf8))
    }

    func testDecodesEveryKindOfTrigger() throws {
        let triggers = try JSONDecoder.cedar.decode([Trigger].self, from: Data("""
        [{"kind":"time","frequency":"weekly","weekday":0,"time":"07:30","timezone":"Mars/Olympus"},
         {"kind":"time","frequency":"weekdays","time":"09:00","timezone":"Europe/Berlin"},
         {"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Berlin"},
         {"kind":"interval","every_minutes":90},
         {"kind":"monthly","day":31,"time":"08:00","timezone":"Mars/Olympus"},
         {"kind":"condition","source":{"type":"url","url":"https://api.example.com/price","as":"json"},
          "extract":"price.eur","predicate":{"op":"gt","value":100},"every_minutes":15},
         {"kind":"condition","source":{"type":"tool","name":"weather_forecast","input":{"city":"Zurich"}},
          "predicate":{"op":"changes"}},
         {"kind":"condition","source":{"type":"records","set":"runs"},"extract":"count",
          "predicate":{"op":"contains","value":"7"},"every_minutes":60},
         {"kind":"webhook"},
         {"kind":"email","query":"from:billing@example.com","every_minutes":5},
         {"kind":"calendar","filter":"standup","minutes_before":15,"new_events":true,"every_minutes":10},
         {"kind":"calendar","calendar_id":"team","new_events":true}]
        """.utf8))
        XCTAssertEqual(triggers, [
            .time(frequency: "weekly", weekday: 0, time: "07:30", timezone: "Mars/Olympus"),
            .time(frequency: "weekdays", weekday: nil, time: "09:00", timezone: "Europe/Berlin"),
            .once(at: "2026-12-03T09:00", timezone: "Europe/Berlin"),
            .interval(everyMinutes: 90),
            .monthly(day: 31, time: "08:00", timezone: "Mars/Olympus"),
            .condition(source: .url(url: "https://api.example.com/price", format: "json"), extract: "price.eur",
                       predicate: Trigger.Predicate(op: "gt", value: .number(100)), everyMinutes: 15),
            // The server's default when a watching trigger doesn't say how often.
            .condition(source: .tool(name: "weather_forecast", input: .object(["city": .string("Zurich")])), extract: nil,
                       predicate: Trigger.Predicate(op: "changes", value: nil), everyMinutes: 5),
            .condition(source: .records(set: "runs"), extract: "count",
                       predicate: Trigger.Predicate(op: "contains", value: .string("7")), everyMinutes: 60),
            .webhook,
            .email(query: "from:billing@example.com", everyMinutes: 5),
            .calendar(calendarId: nil, filter: "standup", minutesBefore: 15, newEvents: true, everyMinutes: 10),
            .calendar(calendarId: "team", filter: nil, minutesBefore: nil, newEvents: true, everyMinutes: 5),
        ])
        XCTAssertEqual(triggers.map(\.kind), ["time", "time", "once", "interval", "monthly", "condition", "condition", "condition",
                                              "webhook", "email", "calendar", "calendar"])
        XCTAssertEqual(triggers.map(\.group), [.schedule, .schedule, .schedule, .schedule, .schedule, .change, .change, .change,
                                               .webhook, .mailAndCalendar, .mailAndCalendar, .mailAndCalendar])
        XCTAssertTrue(triggers[0].isClock && !triggers[0].byEvent && !triggers[0].watches)
        XCTAssertTrue(triggers[5].watches && triggers[5].byEvent)
        XCTAssertTrue(triggers[8].byEvent && !triggers[8].watches && !triggers[8].isClock)
        XCTAssertEqual(triggers[2].timezone, "Europe/Berlin")
        XCTAssertNil(triggers[3].timezone)
    }

    func testATriggerItDoesNotKnowNeverBreaksTheList() throws {
        // A kind from a newer server, a known kind with a field missing, a source it can't read,
        // something that isn't an object, and no trigger at all.
        let list = try JSONDecoder.cedar.decode(AgentList.self, from: Data("""
        {"agents":[{"id":"a1","title":"Moon","prompt":"p","trigger":{"kind":"moon_phase","phase":"full"},"next_run_at":"2026-12-03T09:00:00Z"},
                   {"id":"a2","title":"Broken","trigger":{"kind":"once"}},
                   {"id":"a3","title":"Source","trigger":{"kind":"condition","source":{"type":"sensor","pin":4},"predicate":{"op":"changes"}}},
                   {"id":"a4","title":"Text","trigger":"daily"},
                   {"id":"a5","title":"None"},
                   {"id":"a6","title":"Fine","trigger":{"kind":"webhook"}}]}
        """.utf8))
        XCTAssertEqual(list.agents.map(\.trigger), [.unknown(kind: "moon_phase"), .unknown(kind: "once"), .unknown(kind: "condition"),
                                                    .unknown(kind: ""), .unknown(kind: ""), .webhook])
        XCTAssertFalse(list.agents[0].trigger.isKnown)
        XCTAssertEqual(list.agents[0].trigger.group, .other)
        XCTAssertEqual(list.agents[0].trigger.summary, "A trigger this version of the app doesn't know (moon_phase)")
        XCTAssertEqual(list.agents[0].status, .nextRun(try XCTUnwrap(CedarDate.parse("2026-12-03T09:00:00Z"))))
        // Kept under what it would be, though it can't be read.
        XCTAssertEqual(list.agents[1].trigger.group, .schedule)
        XCTAssertEqual(list.agents[4].status, .idle)
        // No limits sent: the server's documented defaults.
        XCTAssertEqual(list.limits, AgentList.Limits())

        // The form shows such a trigger as it is and never sends it.
        let draft = AgentDraft(agent: list.agents[0])
        XCTAssertEqual(draft.fixedTrigger, .unknown(kind: "moon_phase"))
        var edited = draft
        edited.title = "Full moon"
        let body = try edited.updateBody(for: list.agents[0])
        XCTAssertNil(body.trigger)
        XCTAssertThrowsError(try edited.trigger())
    }

    func testDecodesAnAgentWithAlmostNothing() throws {
        let bare = try agent(#"{"id":"a1","title":"Bare"}"#)
        XCTAssertEqual(bare.prompt, "")
        XCTAssertTrue(bare.enabled)
        XCTAssertFalse(bare.waiting)
        XCTAssertNil(bare.nextRunAt)
        XCTAssertNil(bare.tools)
        XCTAssertNil(bare.hook)
        XCTAssertNil(bare.lastRun)
        XCTAssertEqual(bare.trigger, .unknown(kind: ""))
        XCTAssertThrowsError(try agent(#"{"title":"No id"}"#))
    }

    func testDecodesAgentsRunsAndCauses() throws {
        let list = try JSONDecoder.cedar.decode(AgentList.self, from: Data("""
        {"agents":[{"id":"a1","title":"News","prompt":"Summarize the news","model":null,"reasoning":null,"tools":null,"notify":null,
          "enabled":true,"source_conversation_id":null,"timezone":"Mars/Olympus",
          "trigger":{"kind":"time","frequency":"weekly","weekday":0,"time":"07:30","timezone":"Mars/Olympus"},
          "next_run_at":"2026-10-26T06:30:00Z","next_check_at":null,"last_run_at":"2026-10-19T06:30:00.123456789Z",
          "last_checked_at":null,"last_error":null,"waiting":false,"min_interval_secs":null,"max_runs_per_day":null,"hook":null,
          "created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-01T00:00:00Z",
          "last_run":{"id":"r1","schedule_id":"a1","conversation_id":"c1","turn_id":"t1","due_at":"2026-10-19T06:30:00Z",
            "started_at":"2026-10-19T06:30:05Z","manual":false,"error":null,
            "cause":{"kind":"clock","summary":"on schedule"},
            "turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"x","prompt":"Summarize the news","status":"completed",
                    "final_text":"Done","error":null,"stop_reason":null,"finished_at":"2026-10-19T06:31:00Z","seen":false,
                    "attachments":[],"created_at":"2026-10-19T06:30:05Z"}}},
         {"id":"a2","title":"Door","prompt":"p","enabled":true,"trigger":{"kind":"webhook"},"next_run_at":null,"waiting":true,
          "tools":["weather","gmail_search_threads"],"notify":"ios","min_interval_secs":600,"max_runs_per_day":10,
          "hook":{"url":"https://chat.example/api/hooks/0123","path":"/api/hooks/0123","secret_last4":"abcd"},"last_run":null}],
         "limits":{"max_agents":100,"min_interval_secs":300,"max_runs_per_day":24,"min_interval_minutes":15,"check_minutes":5,
                   "personal_check_minutes":5,"something_new":1}}
        """.utf8))
        XCTAssertEqual(list.agents.count, 2)
        XCTAssertEqual(list.limits.maxAgents, 100)
        XCTAssertEqual(list.limits.minIntervalMinutes, 15)
        let news = list.agents[0]
        XCTAssertEqual(news.trigger.summary, "Mondays at 07:30 (Mars/Olympus)")
        XCTAssertEqual(news.lastRun?.outcome, .completed)
        XCTAssertEqual(news.lastRun?.unread, true)
        XCTAssertEqual(news.lastRun?.cause?.kind, "clock")
        XCTAssertEqual(news.lastRun?.startedBy, "on schedule")
        XCTAssertNotNil(news.lastRunAt)
        let door = list.agents[1]
        XCTAssertEqual(door.hook?.url, "https://chat.example/api/hooks/0123")
        XCTAssertEqual(door.hook?.secretLast4, "abcd")
        XCTAssertNil(door.hook?.secret)
        XCTAssertEqual(door.tools, ["weather", "gmail_search_threads"])
        XCTAssertEqual(door.minIntervalSecs, 600)
        XCTAssertEqual(door.status, .callWaiting)

        // The answer to creating a webhook agent, or to asking for a new secret, carries the secret.
        let made = try agent("""
        {"id":"a2","title":"Door","trigger":{"kind":"webhook"},
         "hook":{"url":"https://chat.example/api/hooks/0123","path":"/api/hooks/0123","secret_last4":"cdef","secret":"00cdef"}}
        """)
        XCTAssertEqual(made.hook?.secret, "00cdef")
        XCTAssertEqual(made.status, .waitingForCalls)

        let detail = try JSONDecoder.cedar.decode(AgentDetail.self, from: Data("""
        {"agent":{"id":"a3","title":"Price","prompt":"p","enabled":true,
                  "trigger":{"kind":"condition","source":{"type":"url","url":"https://api.example.com/price"},
                             "predicate":{"op":"lt","value":40.5},"every_minutes":5},
                  "next_run_at":null,"next_check_at":"2026-10-20T07:35:00Z","last_checked_at":"2026-10-20T07:30:00Z"},
         "runs":[{"id":"r3","schedule_id":"a3","conversation_id":"c3","turn_id":"t3","due_at":"2026-10-20T07:30:00Z",
                  "started_at":"2026-10-20T07:30:01Z","manual":false,"error":null,
                  "cause":{"kind":"holds","summary":"the watched value is 39 (< 40.5)","excerpt":"{\\"price\\":39}","value":"39","op":"lt","target":"40.5"},
                  "turn":null},
                 {"id":"r2","schedule_id":"a3","conversation_id":null,"turn_id":null,"due_at":"2026-10-20T06:30:00Z",
                  "started_at":"2026-10-20T07:30:00Z","manual":true,"error":"cedar is busy","cause":null,"turn":null},
                 {"id":"r1","schedule_id":"a3","started_at":"2026-10-19T06:30:00Z"}]}
        """.utf8))
        XCTAssertEqual(detail.agent.trigger.summary, "api.example.com: the value < 40.5")
        XCTAssertEqual(detail.agent.status, .watching(lastChecked: CedarDate.parse("2026-10-20T07:30:00Z")))
        XCTAssertEqual(detail.runs.map(\.id), ["r3", "r2", "r1"])
        XCTAssertEqual(detail.runs[0].cause?.kind, "holds")
        XCTAssertEqual(detail.runs[0].startedBy, "the watched value is 39 (< 40.5)")
        XCTAssertEqual(detail.runs[0].cause?.excerpt, #"{"price":39}"#)
        // Run Now, and a run from before causes were kept: neither says what started it.
        XCTAssertNil(detail.runs[1].cause)
        XCTAssertNil(detail.runs[1].startedBy)
        XCTAssertTrue(detail.runs[1].manual)
        XCTAssertEqual(detail.runs[1].outcome, .notStarted)
        XCTAssertEqual(detail.runs[1].unread, false)
        XCTAssertNil(detail.runs[1].conversationId)
        XCTAssertNil(detail.runs[2].cause)
        XCTAssertFalse(detail.runs[2].manual)
        XCTAssertEqual(detail.runs[2].dueAt, detail.runs[2].startedAt)
    }

    func testDecodesActivityAndAConversationsAgent() throws {
        let page = try JSONDecoder.cedar.decode(ActivityPage.self, from: Data("""
        {"items":[{"id":"r2","schedule_id":"a2","conversation_id":"c2","turn_id":"t2","due_at":"2026-10-20T07:30:00Z",
                   "started_at":"2026-10-20T07:30:01.25Z","manual":false,"error":null,
                   "cause":{"kind":"webhook","summary":"a webhook was called","bytes":14,"dropped":0},
                   "turn":{"id":"t2","conversation_id":"c2","seq":1,"run_id":"x","prompt":"p","status":"running"},
                   "agent":{"id":"a2","title":"Door","trigger_kind":"webhook"}},
                  {"id":"r1","schedule_id":"a1","conversation_id":"c1","turn_id":"t1","due_at":"2026-10-19T06:30:00Z",
                   "started_at":"2026-10-19T06:30:05Z","manual":true,"error":null,"cause":null,"turn":null,
                   "agent":{"id":"a1","title":"News","trigger_kind":"time"}}],
         "next_before":"2026-10-19T06:30:05.123456+00:00"}
        """.utf8))
        XCTAssertEqual(page.items.map(\.id), ["r2", "r1"])
        XCTAssertEqual(page.items[0].agent, AgentRef(id: "a2", title: "Door", triggerKind: "webhook"))
        XCTAssertEqual(page.items[0].run.startedBy, "a webhook was called")
        XCTAssertTrue(page.items[0].run.isActive)
        XCTAssertNil(page.items[1].run.startedBy)
        XCTAssertTrue(page.items[1].run.manual)
        // Sent back as it came, so the next page starts exactly there.
        XCTAssertEqual(page.nextBefore, "2026-10-19T06:30:05.123456+00:00")
        let last = try JSONDecoder.cedar.decode(ActivityPage.self, from: Data(#"{"items":[],"next_before":null}"#.utf8))
        XCTAssertTrue(last.items.isEmpty)
        XCTAssertNil(last.nextBefore)

        let run = try JSONDecoder.cedar.decode(ConversationDetail.self, from: Data("""
        {"conversation":{"id":"c2","title":"Door · Tue 20 Oct","schedule_id":"a2","unread":false,"needs_answer":false,
                         "created_at":"2026-10-20T07:30:01Z","updated_at":"2026-10-20T07:31:00Z"},
         "turns":[],"agent":{"id":"a2","title":"Door","trigger_kind":"webhook"}}
        """.utf8))
        XCTAssertEqual(run.agent?.title, "Door")
        XCTAssertEqual(run.agent?.triggerKind, "webhook")
        let chat = try JSONDecoder.cedar.decode(ConversationDetail.self, from: Data("""
        {"conversation":{"id":"c1","title":"Trip","schedule_id":null,"unread":false,"needs_answer":false,
                         "created_at":"2026-10-20T07:30:01Z","updated_at":"2026-10-20T07:31:00Z"},"turns":[]}
        """.utf8))
        XCTAssertNil(chat.agent)
    }

    func testDecodesWhatTryingATriggerFound() throws {
        let fires = try JSONDecoder.cedar.decode(TriggerTest.self, from: Data("""
        {"ok":true,"value":"39","would_fire":true,"summary":"the watched value is 39 (< 40.5)",
         "sample":{"text":"{\\n  \\"price\\": 39\\n}","cut":false,"json":true}}
        """.utf8))
        XCTAssertTrue(fires.ok && fires.wouldFire)
        XCTAssertEqual(fires.value, "39")
        XCTAssertEqual(fires.summary, "the watched value is 39 (< 40.5)")
        XCTAssertEqual(fires.sample?.json, true)
        XCTAssertEqual(fires.sample?.text.contains("\"price\": 39"), true)

        // A first look at something that waits for a change only learns the value.
        let learns = try JSONDecoder.cedar.decode(TriggerTest.self, from: Data("""
        {"ok":true,"value":"3 threads","would_fire":false,"summary":null,"sample":{"text":"long…","cut":true,"json":false}}
        """.utf8))
        XCTAssertTrue(learns.ok)
        XCTAssertFalse(learns.wouldFire)
        XCTAssertNil(learns.summary)
        XCTAssertEqual(learns.sample?.cut, true)

        let failed = try JSONDecoder.cedar.decode(TriggerTest.self, from: Data(#"{"ok":false,"error":"the page answered 503"}"#.utf8))
        XCTAssertFalse(failed.ok)
        XCTAssertEqual(failed.error, "the page answered 503")
        XCTAssertNil(failed.sample)
        let noPath = try JSONDecoder.cedar.decode(TriggerTest.self, from: Data("""
        {"ok":false,"error":"nothing at price.usd","sample":{"text":"{}","cut":false,"json":true}}
        """.utf8))
        XCTAssertEqual(noPath.sample?.text, "{}")
    }

    func testTriggersAreSentAsTheServerReadsThem() throws {
        // The server refuses a field it doesn't know, so each kind sends its own and no others.
        let cases: [(Trigger, String)] = [
            (.time(frequency: "weekly", weekday: 6, time: "07:30", timezone: "Europe/Berlin"),
             #"{"kind":"time","frequency":"weekly","weekday":6,"time":"07:30","timezone":"Europe/Berlin"}"#),
            (.time(frequency: "daily", weekday: nil, time: "09:00", timezone: "UTC"),
             #"{"kind":"time","frequency":"daily","time":"09:00","timezone":"UTC"}"#),
            (.once(at: "2026-12-03T09:00", timezone: "Europe/Berlin"),
             #"{"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Berlin"}"#),
            (.interval(everyMinutes: 90), #"{"kind":"interval","every_minutes":90}"#),
            (.monthly(day: 31, time: "08:00", timezone: "Europe/Berlin"),
             #"{"kind":"monthly","day":31,"time":"08:00","timezone":"Europe/Berlin"}"#),
            (.condition(source: .url(url: "https://api.example.com/price", format: "json"), extract: "price.eur",
                        predicate: Trigger.Predicate(op: "gt", value: .number(100)), everyMinutes: 15),
             #"{"kind":"condition","source":{"type":"url","url":"https://api.example.com/price","as":"json"},"extract":"price.eur","predicate":{"op":"gt","value":100},"every_minutes":15}"#),
            (.condition(source: .url(url: "https://example.com", format: nil), extract: nil,
                        predicate: Trigger.Predicate(op: "changes", value: nil), everyMinutes: 5),
             #"{"kind":"condition","source":{"type":"url","url":"https://example.com"},"predicate":{"op":"changes"},"every_minutes":5}"#),
            (.condition(source: .tool(name: "weather_forecast", input: .object(["city": .string("Zurich")])), extract: "/daily/0/rain",
                        predicate: Trigger.Predicate(op: "equals", value: .bool(true)), everyMinutes: 60),
             #"{"kind":"condition","source":{"type":"tool","name":"weather_forecast","input":{"city":"Zurich"}},"extract":"/daily/0/rain","predicate":{"op":"equals","value":true},"every_minutes":60}"#),
            (.condition(source: .records(set: "runs"), extract: "count",
                        predicate: Trigger.Predicate(op: "contains", value: .string("7")), everyMinutes: 5),
             #"{"kind":"condition","source":{"type":"records","set":"runs"},"extract":"count","predicate":{"op":"contains","value":"7"},"every_minutes":5}"#),
            (.webhook, #"{"kind":"webhook"}"#),
            (.email(query: "", everyMinutes: 5), #"{"kind":"email","query":"","every_minutes":5}"#),
            (.calendar(calendarId: nil, filter: "standup", minutesBefore: 15, newEvents: false, everyMinutes: 5),
             #"{"kind":"calendar","filter":"standup","minutes_before":15,"new_events":false,"every_minutes":5}"#),
            (.calendar(calendarId: "team", filter: nil, minutesBefore: nil, newEvents: true, everyMinutes: 10),
             #"{"kind":"calendar","calendar_id":"team","new_events":true,"every_minutes":10}"#),
        ]
        for (trigger, json) in cases {
            XCTAssertEqual(try sent(trigger), try object(json), trigger.kind)
            // And it reads back as it was.
            XCTAssertEqual(try JSONDecoder.cedar.decode(Trigger.self, from: JSONEncoder().encode(trigger)), trigger)
        }
        XCTAssertEqual(try sent(TriggerTestBody(trigger: .email(query: "is:unread", everyMinutes: 5))),
                       try object(#"{"trigger":{"kind":"email","query":"is:unread","every_minutes":5}}"#))
    }

    func testTheFormMakesTheCreateBodyForEveryKind() throws {
        let device = TimeZone.current.identifier
        let limits = AgentList.Limits()
        let now = try XCTUnwrap(CedarDate.parse("2026-11-12T10:00:00Z"))
        var draft = AgentDraft(limits: limits, now: now)
        // Nothing can be saved without a name and a prompt.
        XCTAssertThrowsError(try draft.createBody(now: now)) { XCTAssertEqual($0 as? AgentDraft.Problem, .titleMissing) }
        draft.title = "  Watcher "
        XCTAssertThrowsError(try draft.createBody(now: now)) { XCTAssertEqual($0 as? AgentDraft.Problem, .promptMissing) }
        draft.prompt = "DO: look into it\n"

        func body(_ draft: AgentDraft, conversationId: String? = nil, catalog: ToolCatalog = .empty) throws -> NSDictionary {
            try sent(draft.createBody(conversationId: conversationId, limits: limits, catalog: catalog, now: now))
        }
        func expected(_ trigger: String, _ more: String = "") throws -> NSDictionary {
            try object(#"{"title":"Watcher","prompt":"DO: look into it","model":"","reasoning":"","notify":null,"timezone":"\#(device)","trigger":\#(trigger)\#(more)}"#)
        }

        // Daily or weekly: the time as the zone's clock shows it, a weekday only when weekly.
        draft.clock = try XCTUnwrap(AgentClock.date(time: "07:30", timezone: device, on: now))
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"time","frequency":"daily","time":"07:30","timezone":"\#(device)"}"#))
        draft.weekday = 4
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"time","frequency":"daily","time":"07:30","timezone":"\#(device)"}"#))
        draft.frequency = "weekly"
        XCTAssertEqual(try body(draft, conversationId: "c1"),
                       try expected(#"{"kind":"time","frequency":"weekly","weekday":4,"time":"07:30","timezone":"\#(device)"}"#, #","conversation_id":"c1""#))

        // Once: a local date and time, in the future.
        draft.kind = .once
        draft.timezone = "Europe/Berlin"
        draft.onceAt = try XCTUnwrap(AgentClock.date(at: "2026-12-03T09:00", timezone: "Europe/Berlin"))
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Berlin"}"#))
        draft.onceAt = now.addingTimeInterval(-60)
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .oncePast) }

        // Every so often: no shorter than the server allows, no longer than a week.
        draft.kind = .interval
        draft.interval = AgentDraft.Every(amount: 90, unit: .minutes)
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"interval","every_minutes":90}"#))
        draft.interval = AgentDraft.Every(amount: 2, unit: .days)
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"interval","every_minutes":2880}"#))
        draft.interval = AgentDraft.Every(amount: 5, unit: .minutes)
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .intervalTooShort(15)) }
        draft.interval = AgentDraft.Every(amount: 8, unit: .days)
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .tooLong) }

        // Monthly.
        draft.kind = .monthly
        draft.monthDay = 31
        draft.clock = try XCTUnwrap(AgentClock.date(time: "08:05", timezone: "Europe/Berlin", on: now))
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"monthly","day":31,"time":"08:05","timezone":"Europe/Berlin"}"#))

        // A condition on a web address: a number for a comparison, a decimal comma included.
        draft.kind = .condition
        draft.url = " https://api.example.com/price "
        draft.extract = " price.eur "
        draft.op = "lte"
        draft.value = "40,5"
        draft.check = AgentDraft.Every(amount: 1, unit: .hours)
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"condition","source":{"type":"url","url":"https://api.example.com/price","as":"json"},"extract":"price.eur","predicate":{"op":"lte","value":40.5},"every_minutes":60}"#))
        draft.value = "cheap"
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .numberNeeded) }
        draft.op = "contains"
        draft.urlFormat = "text"
        draft.extract = ""
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"condition","source":{"type":"url","url":"https://api.example.com/price","as":"text"},"predicate":{"op":"contains","value":"cheap"},"every_minutes":60}"#))
        draft.value = " "
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .valueNeeded) }
        draft.url = "example.com"
        draft.op = "changes"
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .urlInvalid) }

        // On a tool: its input as JSON (the keyboard's curly quotes included); a personal tool is checked less often.
        let catalog = try JSONDecoder.cedar.decode(ToolCatalog.self, from: Data(Self.catalog.utf8))
        draft.sourceType = .tool
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .toolMissing) }
        draft.toolName = "weather_forecast"
        draft.toolInput = "{\u{201C}city\u{201D}: \u{201C}Zurich\u{201D}}"
        draft.check = AgentDraft.Every(amount: 1, unit: .minutes)
        XCTAssertEqual(try body(draft, catalog: catalog), try expected(#"{"kind":"condition","source":{"type":"tool","name":"weather_forecast","input":{"city":"Zurich"}},"predicate":{"op":"changes"},"every_minutes":1}"#))
        draft.toolInput = ""
        XCTAssertEqual(try body(draft, catalog: catalog), try expected(#"{"kind":"condition","source":{"type":"tool","name":"weather_forecast","input":{}},"predicate":{"op":"changes"},"every_minutes":1}"#))
        draft.toolInput = "[1]"
        XCTAssertThrowsError(try body(draft, catalog: catalog)) { XCTAssertEqual($0 as? AgentDraft.Problem, .toolInputInvalid) }
        draft.toolInput = "{}"
        draft.toolName = "air_quality"
        XCTAssertThrowsError(try body(draft, catalog: catalog)) { XCTAssertEqual($0 as? AgentDraft.Problem, .checkTooOften(5)) }
        draft.check = AgentDraft.Every(amount: 0, unit: .minutes)
        XCTAssertThrowsError(try body(draft, catalog: catalog)) { XCTAssertEqual($0 as? AgentDraft.Problem, .amountMissing) }

        // On a record set.
        draft.sourceType = .records
        draft.check = AgentDraft.Every(amount: 5, unit: .minutes)
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .recordSetMissing) }
        draft.recordSet = "runs"
        draft.extract = "count"
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"condition","source":{"type":"records","set":"runs"},"extract":"count","predicate":{"op":"changes"},"every_minutes":5}"#))

        // A webhook has nothing to fill in. Limits go only when the agent sets its own, and the
        // tools as chosen: a careful list, the defaults (`null`), or picked ones.
        draft.kind = .webhook
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"webhook"}"#))
        draft.customLimits = true
        draft.minInterval = AgentDraft.Every(amount: 10, unit: .minutes)
        draft.maxRunsPerDay = 12
        draft.notify = "ios"
        draft.model = "deepseek"
        draft.reasoning = "high"
        draft.toolsChoice = .all
        XCTAssertEqual(try body(draft), try object(#"{"title":"Watcher","prompt":"DO: look into it","model":"deepseek","reasoning":"high","notify":"ios","timezone":"\#(device)","trigger":{"kind":"webhook"},"tools":null,"min_interval_secs":600,"max_runs_per_day":12}"#))
        draft.customLimits = false
        draft.notify = ""
        draft.model = ""
        draft.reasoning = ""
        draft.toolsChoice = .careful
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .toolsUnavailable) }
        XCTAssertEqual(try body(draft, catalog: catalog), try expected(#"{"kind":"webhook"}"#, #","tools":["calendar_list_events","weather"]"#))
        draft.toolsChoice = .custom
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .noTools) }
        draft.customTools = ["web_fetch", "calendar_create_event"]
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"webhook"}"#, #","tools":["calendar_create_event","web_fetch"]"#))
        draft.toolsChoice = .automatic

        // Mail: Gmail's search, or empty for any mail.
        draft.kind = .email
        draft.query = " from:billing@example.com "
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"email","query":"from:billing@example.com","every_minutes":5}"#))
        draft.query = ""
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"email","query":"","every_minutes":5}"#))

        // Calendar: before events, new events, or both, but one of them.
        draft.kind = .calendar
        draft.calendarFilter = " standup "
        draft.before = AgentDraft.Every(amount: 1, unit: .hours)
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"calendar","filter":"standup","minutes_before":60,"new_events":false,"every_minutes":5}"#))
        draft.newEvents = true
        draft.remindBefore = false
        draft.calendarFilter = ""
        XCTAssertEqual(try body(draft), try expected(#"{"kind":"calendar","new_events":true,"every_minutes":5}"#))
        draft.newEvents = false
        XCTAssertThrowsError(try body(draft)) { XCTAssertEqual($0 as? AgentDraft.Problem, .calendarNeedsOne) }

        // Only what watches can be tried out.
        XCTAssertTrue(draft.canTest && draft.byEvent)
        draft.kind = .webhook
        XCTAssertTrue(!draft.canTest && draft.byEvent)
        draft.kind = .time
        XCTAssertTrue(!draft.canTest && !draft.byEvent)
    }

    private static let catalog = """
    {"groups":[{"id":"weather","label":"Weather","available":true,"tools":[{"name":"weather_forecast","description":"d","writes":false,"pollable":true}]},
               {"id":"web_fetch","label":"Read web pages","available":false,"tools":[{"name":"fetch_url","description":"d","writes":false,"pollable":false}]},
               {"id":"google","label":"Google","available":true,"tools":[
                 {"name":"calendar_list_events","description":"d","writes":false,"pollable":true},
                 {"name":"calendar_create_event","description":"d","writes":true,"pollable":false}]},
               {"id":"mcp","label":"MCP","available":true,"discovered":false,"tools":[]},
               {"id":"custom","label":"Personal tools","available":true,"tools":[{"name":"air_quality","writes":false,"pollable":true}]}],
     "sources":[{"name":"url","description":"a web address"}]}
    """

    func testDecodesTheToolCatalogAndNotificationMethods() throws {
        let catalog = try JSONDecoder.cedar.decode(ToolCatalog.self, from: Data(Self.catalog.utf8))
        XCTAssertEqual(catalog.groups.map(\.id), ["weather", "web_fetch", "google", "mcp", "custom"])
        XCTAssertEqual(catalog.pollableTools.map(\.name), ["weather_forecast", "calendar_list_events", "air_quality"])
        XCTAssertTrue(catalog.isPersonal(tool: "air_quality"))
        XCTAssertFalse(catalog.isPersonal(tool: "weather_forecast"))
        // As the server chooses for an agent that events start: no personal tools, none that
        // write, no group that isn't set up or whose tools aren't known yet.
        XCTAssertEqual(catalog.carefulTools, ["calendar_list_events", "weather"])
        XCTAssertTrue(try JSONDecoder.cedar.decode(ToolCatalog.self, from: Data("{}".utf8)).groups.isEmpty)

        let status = try JSONDecoder.cedar.decode(NotificationStatus.self, from: Data("""
        {"default_method":"ios","configured":["desktop","ios"],"scope":"attention","ios":{"configured":true,"devices":[]}}
        """.utf8))
        XCTAssertEqual(status.defaultMethod, "ios")
        XCTAssertEqual(status.configured, ["desktop", "ios"])
        XCTAssertEqual(NotificationStatus.label("ios"), "iPhone push")
        XCTAssertEqual(NotificationStatus.label("carrier_pigeon"), "carrier_pigeon")
        XCTAssertEqual(try JSONDecoder.cedar.decode(Compacted.self, from: Data(#"{"prompt":"DO: it","source":"run"}"#.utf8)).prompt, "DO: it")
    }

    func testAChangeSendsOnlyWhatIsSet() throws {
        // Pausing sends one field.
        XCTAssertEqual(try sent(AgentBody(enabled: false)), try object(#"{"enabled":false}"#))
        XCTAssertEqual(try sent(AgentBody()), try object("{}"))
        // `null` is sent, not left out, where it means "back to the default".
        var body = AgentBody(title: "Renamed")
        body.tools = .null
        body.notify = .null
        body.minIntervalSecs = .null
        body.maxRunsPerDay = .value(3)
        let json = try sent(body)
        XCTAssertEqual(json, try object(#"{"title":"Renamed","tools":null,"notify":null,"min_interval_secs":null,"max_runs_per_day":3}"#))
        XCTAssertTrue(json["tools"] is NSNull)
        XCTAssertNil(json["trigger"])

        // Editing in the form: the trigger goes only when it was changed, since sending one makes
        // the server forget what the old one had seen.
        let device = TimeZone.current.identifier
        let watcher = try agent("""
        {"id":"a3","title":"Price","prompt":"Tell me","model":"deepseek","reasoning":"high","notify":"ios","tools":["weather"],
         "enabled":true,"min_interval_secs":600,"max_runs_per_day":10,
         "trigger":{"kind":"condition","source":{"type":"url","url":"https://api.example.com/price","as":"json"},
                    "extract":"price","predicate":{"op":"equals","value":40},"every_minutes":120}}
        """)
        var draft = AgentDraft(agent: watcher)
        XCTAssertEqual(draft.kind, .condition)
        XCTAssertEqual(draft.value, "40")
        XCTAssertEqual(draft.check, AgentDraft.Every(amount: 2, unit: .hours))
        XCTAssertTrue(draft.customLimits)
        XCTAssertEqual(draft.minInterval, AgentDraft.Every(amount: 10, unit: .minutes))
        draft.prompt = "Tell me at once"
        XCTAssertEqual(try sent(draft.updateBody(for: watcher)), try object(#"{"title":"Price","prompt":"Tell me at once","model":"deepseek","reasoning":"high","notify":"ios","timezone":"\#(device)","min_interval_secs":600,"max_runs_per_day":10}"#))

        // The default model and method, the server's limits and tools again, and another test.
        draft.model = ""
        draft.reasoning = ""
        draft.notify = ""
        draft.customLimits = false
        draft.toolsChoice = .all
        draft.op = "gt"
        XCTAssertEqual(try sent(draft.updateBody(for: watcher)), try object(#"{"title":"Price","prompt":"Tell me at once","model":"","reasoning":"","notify":null,"timezone":"\#(device)","tools":null,"min_interval_secs":null,"max_runs_per_day":null,"trigger":{"kind":"condition","source":{"type":"url","url":"https://api.example.com/price","as":"json"},"extract":"price","predicate":{"op":"gt","value":40},"every_minutes":120}}"#))

        // A one-off that has run can still be renamed: its past time isn't checked unless changed.
        let done = try agent("""
        {"id":"a4","title":"Once","prompt":"p","enabled":false,"next_run_at":null,
         "trigger":{"kind":"once","at":"2020-01-01T09:00","timezone":"Europe/Berlin"}}
        """)
        var renamed = AgentDraft(agent: done)
        XCTAssertEqual(renamed.timezone, "Europe/Berlin")
        renamed.title = "Once more"
        XCTAssertNil(try renamed.updateBody(for: done).trigger)
        renamed.kind = .interval
        XCTAssertEqual(try renamed.updateBody(for: done).trigger, .interval(everyMinutes: 60))
        // A clock agent has no limits to send.
        XCTAssertEqual(try sent(renamed.updateBody(for: done)), try object(#"{"title":"Once more","prompt":"p","model":"","reasoning":"","notify":null,"timezone":"\#(device)","trigger":{"kind":"interval","every_minutes":60}}"#))
    }

    func testAnAgentsStatusAndWhatItsTriggerSays() throws {
        // Paused comes first, whatever else is true; a one-off that has run is done.
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","enabled":false,"last_error":"x","trigger":{"kind":"interval","every_minutes":30},"next_run_at":"2026-12-03T09:00:00Z"}"#).status, .paused)
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","enabled":false,"next_run_at":null,"trigger":{"kind":"once","at":"2026-01-01T09:00","timezone":"UTC"}}"#).status, .done)
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","enabled":false,"next_run_at":"2026-12-03T09:00:00Z","trigger":{"kind":"once","at":"2026-12-03T09:00","timezone":"UTC"}}"#).status, .paused)
        // Then a failing check, with the server's reason.
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","enabled":true,"last_error":"the page answered 503","last_checked_at":"2026-12-03T09:00:00Z","trigger":{"kind":"email","query":""}}"#).status,
                       .checkFailing("the page answered 503"))
        // A clock says when it runs next; what watches, when it last looked; a webhook waits.
        let next = try XCTUnwrap(CedarDate.parse("2026-12-03T09:00:00Z"))
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","trigger":{"kind":"monthly","day":1,"time":"09:00","timezone":"UTC"},"next_run_at":"2026-12-03T09:00:00Z"}"#).status, .nextRun(next))
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","trigger":{"kind":"interval","every_minutes":30}}"#).status, .idle)
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","trigger":{"kind":"calendar","new_events":true},"last_checked_at":"2026-12-03T09:00:00Z","next_check_at":"2026-12-03T09:05:00Z"}"#).status,
                       .watching(lastChecked: next))
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","trigger":{"kind":"email","query":""}}"#).status, .watching(lastChecked: nil))
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","trigger":{"kind":"webhook"}}"#).status, .waitingForCalls)
        XCTAssertEqual(try agent(#"{"id":"a","title":"t","trigger":{"kind":"webhook"},"waiting":true}"#).status, .callWaiting)

        // One line per trigger, with the time zone when it isn't this device's.
        XCTAssertEqual(Trigger.time(frequency: "weekdays", weekday: nil, time: "07:30", timezone: "Mars/Olympus").summary, "Weekdays at 07:30 (Mars/Olympus)")
        XCTAssertEqual(Trigger.time(frequency: "daily", weekday: nil, time: "07:30", timezone: TimeZone.current.identifier).summary, "Daily at 07:30")
        XCTAssertEqual(Trigger.time(frequency: "weekly", weekday: 6, time: "18:00", timezone: "Mars/Olympus").summary, "Sundays at 18:00 (Mars/Olympus)")
        let once = Trigger.once(at: "2026-12-03T09:00", timezone: "Europe/Berlin").summary
        XCTAssertTrue(once.hasPrefix("Once, ") && once.contains("2026"), once)
        XCTAssertEqual(Trigger.once(at: "soon", timezone: "Mars/Olympus").summary, "Once, soon (Mars/Olympus)")
        XCTAssertEqual(Trigger.interval(everyMinutes: 1).summary, "Every minute")
        XCTAssertEqual(Trigger.interval(everyMinutes: 90).summary, "Every 90 minutes")
        XCTAssertEqual(Trigger.interval(everyMinutes: 60).summary, "Every hour")
        XCTAssertEqual(Trigger.interval(everyMinutes: 120).summary, "Every 2 hours")
        XCTAssertEqual(Trigger.interval(everyMinutes: 1440).summary, "Every day")
        XCTAssertEqual(Trigger.interval(everyMinutes: 10080).summary, "Every 7 days")
        XCTAssertEqual(Trigger.monthly(day: 31, time: "08:00", timezone: "Mars/Olympus").summary, "Day 31 of every month at 08:00 (Mars/Olympus)")
        XCTAssertEqual(Trigger.condition(source: .url(url: "https://api.example.com/price", format: "json"), extract: "price.eur",
                                         predicate: Trigger.Predicate(op: "gt", value: .number(100)), everyMinutes: 5).summary,
                       "api.example.com: price.eur > 100")
        XCTAssertEqual(Trigger.condition(source: .tool(name: "weather_forecast", input: .object([:])), extract: nil,
                                         predicate: Trigger.Predicate(op: "changes", value: nil), everyMinutes: 5).summary,
                       "weather_forecast: the value changes")
        XCTAssertEqual(Trigger.condition(source: .records(set: "runs"), extract: "count",
                                         predicate: Trigger.Predicate(op: "contains", value: .string("7")), everyMinutes: 5).summary,
                       "Records runs: count contains 7")
        XCTAssertEqual(Trigger.webhook.summary, "When its webhook is called")
        XCTAssertEqual(Trigger.email(query: " ", everyMinutes: 5).summary, "Any new mail")
        XCTAssertEqual(Trigger.email(query: "is:unread", everyMinutes: 5).summary, "New mail matching is:unread")
        XCTAssertEqual(Trigger.calendar(calendarId: nil, filter: "standup", minutesBefore: 15, newEvents: true, everyMinutes: 5).summary,
                       "15 minutes before an event · When an event is added · matching standup")
        XCTAssertEqual(Trigger.calendar(calendarId: nil, filter: nil, minutesBefore: 0, newEvents: false, everyMinutes: 5).summary, "When an event starts")
        XCTAssertEqual(Trigger.calendar(calendarId: nil, filter: nil, minutesBefore: 120, newEvents: false, everyMinutes: 5).summary, "2 hours before an event")
        XCTAssertEqual(Trigger.unknown(kind: "").summary, "A trigger this version of the app doesn't know")
        XCTAssertEqual(Trigger.Predicate(op: "lte", value: .number(40.5)).text(subject: "price"), "price ≤ 40.5")
        XCTAssertEqual(Trigger.Predicate(op: "equals", value: .bool(true)).valueText, "true")
        XCTAssertEqual(Trigger.Group.allCases.map(\.title), ["On a schedule", "When something changes", "When a webhook is called", "Mail and calendar", "Other"])

        // Times as triggers write them, in the trigger's own zone.
        let nine = try XCTUnwrap(AgentClock.date(at: "2026-12-03T09:00", timezone: "Europe/Berlin"))
        XCTAssertEqual(nine, CedarDate.parse("2026-12-03T08:00:00Z"))
        XCTAssertEqual(AgentClock.at(nine, timezone: "Europe/Berlin"), "2026-12-03T09:00")
        XCTAssertEqual(AgentClock.time(nine, timezone: "UTC"), "08:00")
        XCTAssertNil(AgentClock.date(time: "25:00", timezone: "UTC"))
        XCTAssertEqual(AgentDraft.Every(minutes: 120), AgentDraft.Every(amount: 2, unit: .hours))
        XCTAssertEqual(AgentDraft.Every(minutes: 90).minutes, 90)

        // An agent made in another zone keeps its clock time when moved to this device's.
        var draft = AgentDraft(agent: try agent(#"{"id":"a","title":"t","prompt":"p","trigger":{"kind":"time","frequency":"daily","time":"07:30","timezone":"Pacific/Kiritimati"}}"#))
        if TimeZone.current.identifier != "Pacific/Kiritimati" {
            XCTAssertTrue(draft.hasForeignTimeZone)
            draft.useDeviceTimeZone()
            XCTAssertFalse(draft.hasForeignTimeZone)
            XCTAssertEqual(try draft.trigger(), .time(frequency: "daily", weekday: nil, time: "07:30", timezone: TimeZone.current.identifier))
        }
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
