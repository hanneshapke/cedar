package dev.cedar.core

import okhttp3.HttpUrl.Companion.toHttpUrl
import java.io.IOException
import java.net.UnknownServiceException
import javax.net.ssl.SSLHandshakeException
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

// Ported from iOS/CedarTests/APITests.swift.
class ApiTest {
    @Test
    fun configSaysWhetherTheServerHasMemory() {
        // A server from before memory leaves the field out.
        val old = CedarJson.decodeFromString(ServerConfig.serializer(), """{"auth_required":true,"admin_enabled":false}""")
        assertFalse(old.memory)
        val new = CedarJson.decodeFromString(ServerConfig.serializer(), """{"auth_required":false,"admin_enabled":false,"memory":true}""")
        assertTrue(new.memory)
    }

    @Test
    fun decodesRecordsAndTheConfigFlag() {
        val old = CedarJson.decodeFromString(ServerConfig.serializer(), """{"auth_required":true,"admin_enabled":false,"memory":true}""")
        assertFalse(old.records)
        val new = CedarJson.decodeFromString(ServerConfig.serializer(), """{"auth_required":false,"admin_enabled":false,"memory":true,"records":true}""")
        assertTrue(new.records)

        val list = CedarJson.decodeFromString(RecordsList.serializer(), """
        {"supported":true,"limits":{"max_sets":50,"max_items":10000,"max_record_chars":8000,"max_batch":500},"sets":[
          {"id":"s1","name":"running_log","description":"Weekly runs","key_fields":["date"],"health_data":false,"count":2,
           "columns":["date","km"],"created_at":"2026-10-29T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"},
          {"id":"s2","name":"weight","description":"","key_fields":[],"health_data":true,"count":0,"columns":[],
           "created_at":"2026-10-29T08:00:00Z","updated_at":"2026-10-29T08:00:00Z"}]}
        """)
        assertTrue(list.supported)
        assertEquals(50, list.limits?.maxSets)
        assertEquals(listOf("running_log", "weight"), list.sets.map { it.name })
        assertEquals(listOf("date"), list.sets[0].keyFields)
        assertEquals(listOf("date", "km"), list.sets[0].columns)
        assertTrue(list.sets[1].healthData)

        val detail = CedarJson.decodeFromString(RecordSetDetail.serializer(), """
        {"set":{"id":"s1","name":"running_log","description":"Weekly runs","key_fields":["date"],"health_data":false,"count":2,
                "columns":["date","km"],"created_at":"2026-10-29T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"},
         "records":[{"id":"r1","key":"2026-10-26","data":{"date":"2026-10-26","km":31.2},"run_id":"run-1","conversation_id":"c1","title":"Week 43",
                     "created_at":"2026-10-26T08:00:00Z","updated_at":"2026-10-26T08:00:00Z"},
                    {"id":"r2","key":"2026-11-02","data":{"date":"2026-11-02","km":35,"note":null},"run_id":null,"conversation_id":null,"title":null,
                     "created_at":"2026-11-02T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"}]}
        """)
        assertEquals("running_log", detail.set.name)
        assertEquals(listOf("r1", "r2"), detail.records.map { it.id })
        assertEquals("c1", detail.records[0].conversationId)
        assertEquals(31.2, detail.records[0].data["km"].dbl)
        assertEquals(null, detail.records[1].conversationId)
        assertEquals(null, detail.records[1].runId)
    }

    @Test
    fun decodesConversationsAndTurns() {
        val json = """
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
        val detail = CedarJson.decodeFromString(ConversationDetail.serializer(), json)
        assertEquals("Trip", detail.conversation.displayTitle)
        assertTrue(detail.conversation.unread && detail.conversation.needsAnswer)
        assertEquals(listOf("web_search"), detail.conversation.tools)
        assertEquals(1_791_776_160_500L, detail.conversation.createdAt.toEpochMilli())
        assertEquals(2, detail.turns.size)
        assertTrue(detail.turns[0].needsAnswer)
        assertFalse(detail.turns[0].isActive)
        assertEquals("application/pdf", detail.turns[0].attachments.first().mediaType)
        assertTrue(detail.turns[1].isActive)
        assertFalse(detail.turns[1].needsAnswer)
        val now = java.time.Instant.now()
        assertEquals("New chat", Conversation(id = "x", createdAt = now, updatedAt = now).displayTitle)
        assertEquals("Neuer Chat", Conversation(id = "x", title = " ", createdAt = now, updatedAt = now).titleOr("Neuer Chat"))
    }

    @Test
    fun decodesStarredConversations() {
        val list = CedarJson.decodeFromString(ConversationList.serializer(), """
        {"conversations":[{"id":"c2","title":"Morning digest · Fri 23 Oct","model":null,"reasoning":null,"schedule_id":"s1",
           "tools":null,"unread":false,"needs_answer":false,"starred_at":"2026-10-23T08:00:00Z","schedule_title":"Morning digest",
           "created_at":"2026-10-23T07:00:00Z","updated_at":"2026-10-23T07:01:00Z"},
          {"id":"c1","title":"Trip","model":null,"reasoning":null,"schedule_id":null,"tools":null,"unread":true,
           "needs_answer":false,"starred_at":"2026-10-22T08:00:00Z","schedule_title":null,
           "created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:40:00Z"}]}
        """)
        assertEquals(listOf("c2", "c1"), list.conversations.map { it.id })
        assertTrue(list.conversations.all { it.isStarred })
        assertEquals("Morning digest", list.conversations[0].scheduleTitle)
        assertNull(list.conversations[1].scheduleTitle)

        // The main list sends `starred_at` but no `schedule_title`; older servers send neither.
        val plain = CedarJson.decodeFromString(Conversation.serializer(), """
        {"id":"c3","title":null,"model":null,"reasoning":null,"schedule_id":null,"tools":null,"unread":false,
         "needs_answer":false,"created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:40:00Z"}
        """)
        assertFalse(plain.isStarred)
    }

    @Test
    fun decodesInboxNoticesAndModels() {
        val inbox = CedarJson.decodeFromString(Inbox.serializer(), """
        {"items":[{"turn_id":"t1","conversation_id":"c1","title":"Trip","schedule_id":null,"status":"failed",
                   "stop_reason":null,"needs_answer":false,"finished_at":"2026-10-12T03:40:00Z"}],"unread":1}
        """)
        assertEquals(1, inbox.unread)
        assertEquals("failed", inbox.items.first().status)

        val notice = CedarJson.decodeFromString(Notice.serializer(), """
        {"type":"turn.finished","turn_id":"t1","conversation_id":"c1","run_id":"r1","status":"completed",
         "stop_reason":"ask_user","error":null,"needs_answer":true,"finished_at":"2026-10-12T03:40:00Z","title":"Trip","schedule_id":null}
        """)
        assertEquals("c1", notice.conversationId)
        assertTrue(notice.needsAnswer)

        val models = CedarJson.decodeFromString(ModelList.serializer(), """
        {"models":[{"id":"deepseek","label":"DeepSeek V4","model":"deepseek-ai/DeepSeek-V4-Flash","reasoning_levels":["low","high"],
                    "default_reasoning":"low","multimodal":false,"is_default":true,"price":{"input_usd_per_m":1,"output_usd_per_m":2}},
                   {"id":"bare"}]}
        """)
        assertEquals(listOf("deepseek", "bare"), models.models.map { it.id })
        assertEquals(listOf("low", "high"), models.models[0].reasoningLevels)
        assertTrue(models.models[0].isDefault)
        assertEquals("bare", models.models[1].label)
    }

    @Test
    fun decodesAgentsAndRuns() {
        val list = CedarJson.decodeFromString(AgentList.serializer(), """
        {"agents":[{"id":"a1","title":"News","prompt":"Summarize the news","model":null,"reasoning":null,
          "tools":["weather","gmail_search_threads"],"notify":"android","enabled":true,"source_conversation_id":null,"timezone":"Mars/Olympus",
          "trigger":{"kind":"time","frequency":"weekly","weekday":0,"time":"07:30","timezone":"Mars/Olympus"},
          "next_run_at":"2026-10-26T06:30:00Z","next_check_at":null,"last_run_at":"2026-10-19T06:30:00Z","last_checked_at":null,
          "last_error":null,"waiting":false,"min_interval_secs":null,"max_runs_per_day":null,"hook":null,
          "created_at":"2026-10-01T00:00:00Z","updated_at":"2026-10-01T00:00:00.123456Z",
          "last_run":{"id":"r1","schedule_id":"a1","conversation_id":"c1","turn_id":"t1","due_at":"2026-10-19T06:30:00Z",
            "started_at":"2026-10-19T06:30:05Z","manual":false,"error":null,
            "cause":{"kind":"clock","summary":"On schedule"},
            "turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"x","prompt":"Summarize the news","status":"completed",
                    "final_text":"Done","error":null,"stop_reason":null,"finished_at":"2026-10-19T06:31:00Z","seen":false,
                    "attachments":[],"created_at":"2026-10-19T06:30:05Z"}}},
         {"id":"a2","title":"Bare"}],
         "limits":{"max_agents":100,"min_interval_secs":300,"max_runs_per_day":24,"min_interval_minutes":15,"check_minutes":5,"personal_check_minutes":5}}
        """)
        assertEquals(2, list.agents.size)
        assertEquals(100, list.limits.maxAgents)
        assertEquals(15, list.limits.minIntervalMinutes)
        val news = list.agents[0]
        assertEquals(Trigger.Time("weekly", 0, "07:30", "Mars/Olympus"), news.trigger)
        assertEquals(TriggerGroup.SCHEDULE, news.trigger.group)
        assertEquals("Mars/Olympus", news.zoneLabel(deviceZone = "UTC"))
        assertNull(news.zoneLabel(deviceZone = "Mars/Olympus"))
        assertEquals(listOf("weather", "gmail_search_threads"), news.tools)
        assertEquals(AgentRun.Outcome.COMPLETED, news.lastRun?.outcome)
        assertEquals(true, news.lastRun?.unread)
        assertEquals("On schedule", news.lastRun?.startedBy)
        assertEquals(Agent.Status.NextRun(CedarDate.parse("2026-10-26T06:30:00Z")), news.status)

        // An agent with almost everything missing still lists: an older or a newer server.
        val bare = list.agents[1]
        assertEquals("Bare", bare.title)
        assertTrue(bare.enabled)
        assertNull(bare.nextRunAt)
        assertNull(bare.lastRun)
        assertNull(bare.hook)
        assertEquals(Trigger.Unknown(""), bare.trigger)
        assertEquals(TriggerGroup.OTHER, bare.trigger.group)
        assertEquals(Agent.Status.Active, bare.status)
        assertEquals(AgentLimits(), CedarJson.decodeFromString(AgentList.serializer(), """{"agents":[]}""").limits)

        val detail = CedarJson.decodeFromString(AgentDetail.serializer(), """
        {"agent":{"id":"a1","title":"Door","prompt":"p","enabled":true,"trigger":{"kind":"webhook"},"next_run_at":null,"waiting":true,
                  "hook":{"url":"https://cedar.example/api/hooks/0123456789abcdef0123456789abcdef","path":"/api/hooks/0123456789abcdef0123456789abcdef","secret_last4":"beef"}},
         "runs":[{"id":"r3","schedule_id":"a1","conversation_id":"c3","turn_id":"t3","due_at":"2026-10-21T06:30:00Z",
                  "started_at":"2026-10-21T06:30:00Z","manual":false,"error":null,
                  "cause":{"kind":"webhook","summary":"A webhook call (18 bytes)","excerpt":"{\"door\":\"open\"}","bytes":18,"dropped":0},"turn":null},
                 {"id":"r2","schedule_id":"a1","conversation_id":null,"turn_id":null,"due_at":"2026-10-20T06:30:00Z",
                  "started_at":"2026-10-20T07:30:00Z","manual":true,"error":"cedar is busy","cause":null,"turn":null},
                 {"id":"r1","schedule_id":"a1"}]}
        """)
        assertEquals(Trigger.Webhook, detail.agent.trigger)
        assertEquals(Agent.Status.CallWaiting, detail.agent.status)
        assertEquals("beef", detail.agent.hook?.secretLast4)
        assertNull(detail.agent.hook?.secret)
        assertEquals("webhook", detail.runs[0].cause?.kind)
        assertEquals("A webhook call (18 bytes)", detail.runs[0].startedBy)
        assertEquals("""{"door":"open"}""", detail.runs[0].cause?.excerpt)
        // Run now, and a run from before triggers, have no cause.
        assertTrue(detail.runs[1].manual)
        assertNull(detail.runs[1].cause)
        assertNull(detail.runs[1].startedBy)
        assertEquals(AgentRun.Outcome.NOT_STARTED, detail.runs[1].outcome)
        assertEquals(false, detail.runs[1].unread)
        assertNull(detail.runs[1].conversationId)
        assertNull(detail.runs[2].time)

        // The secret comes once, with the answer to creating the agent or to a new secret.
        val made = CedarJson.decodeFromString(Agent.serializer(), """
        {"id":"a1","title":"Door","trigger":{"kind":"webhook"},
         "hook":{"url":"https://cedar.example/api/hooks/ab","path":"/api/hooks/ab","secret_last4":"cdef","secret":"0123cdef"}}
        """)
        assertEquals("0123cdef", made.hook?.secret)
        assertEquals(
            """curl -X POST -H "Authorization: Bearer 0123cdef" -H "content-type: application/json" -d '{"door":"open"}' https://cedar.example/api/hooks/ab""",
            made.hook?.curlExample(),
        )
        assertTrue(detail.agent.hook!!.curlExample().contains("Bearer <secret>"))
    }

    @Test
    fun decodesEveryKindOfTrigger() {
        fun trigger(json: String): Trigger = CedarJson.decodeFromString(Agent.serializer(), """{"id":"a","title":"t","trigger":$json}""").trigger

        assertEquals(Trigger.Time("daily", null, "09:00", "UTC"), trigger("""{"kind":"time","frequency":"daily","time":"09:00","timezone":"UTC"}"""))
        assertEquals(Trigger.Once("2026-12-03T09:00", "Europe/Berlin"), trigger("""{"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Berlin"}"""))
        assertEquals(Trigger.Interval(90), trigger("""{"kind":"interval","every_minutes":90}"""))
        assertEquals(Trigger.Monthly(31, "18:15", "UTC"), trigger("""{"kind":"monthly","day":31,"time":"18:15","timezone":"UTC"}"""))
        assertEquals(
            Trigger.Condition(WatchSource.Url("https://example.com/price", "json"), "/price/eur", Predicate("lt", j("40.0")), 10),
            trigger("""{"kind":"condition","source":{"type":"url","url":"https://example.com/price","as":"json"},"extract":"/price/eur",
                        "predicate":{"op":"lt","value":40.0},"every_minutes":10}"""),
        )
        val tool = trigger("""{"kind":"condition","source":{"type":"tool","name":"weather","input":{"place":"Bern"}},
                               "predicate":{"op":"changes"},"every_minutes":5}""") as Trigger.Condition
        assertEquals(WatchSource.Tool("weather", j("""{"place":"Bern"}""") as kotlinx.serialization.json.JsonObject), tool.source)
        assertEquals(Predicate("changes"), tool.predicate)
        assertNull(tool.extract)
        val records = trigger("""{"kind":"condition","source":{"type":"records","set":"running_log"},"extract":"count",
                                  "predicate":{"op":"gte","value":10}}""") as Trigger.Condition
        assertEquals(WatchSource.Records("running_log"), records.source)
        assertEquals(5, records.everyMinutes, "the server's default")
        assertEquals("10", records.predicate.valueText)
        assertEquals("40", Predicate("lt", j("40.0")).valueText)
        assertEquals("sold out", Predicate("contains", j("\"sold out\"")).valueText)
        assertEquals(Trigger.Webhook, trigger("""{"kind":"webhook"}"""))
        assertEquals(Trigger.Email("from:boss@example.com is:unread", 5), trigger("""{"kind":"email","query":"from:boss@example.com is:unread","every_minutes":5}"""))
        assertEquals(
            Trigger.Calendar(null, "standup", 15, true, 5),
            trigger("""{"kind":"calendar","filter":"standup","minutes_before":15,"new_events":true,"every_minutes":5}"""),
        )
        assertEquals(Trigger.Calendar(calendarId = "work", newEvents = true), trigger("""{"kind":"calendar","calendar_id":"work","new_events":true}"""))

        assertEquals(
            listOf(TriggerGroup.SCHEDULE, TriggerGroup.SCHEDULE, TriggerGroup.SCHEDULE, TriggerGroup.SCHEDULE, TriggerGroup.CONDITION, TriggerGroup.WEBHOOK, TriggerGroup.MAIL_CALENDAR, TriggerGroup.MAIL_CALENDAR),
            listOf("time", "once", "interval", "monthly", "condition", "webhook", "email", "calendar").map { TriggerKind.of(it)!!.group },
        )
        assertTrue(tool.watches && tool.byEvent && !tool.isClock)
        assertTrue(Trigger.Webhook.byEvent && !Trigger.Webhook.watches)
        assertTrue(Trigger.Interval(15).isClock && !Trigger.Interval(15).byEvent)
        assertEquals("Europe/Berlin", Trigger.Once("2026-12-03T09:00", "Europe/Berlin").timezone)
        assertNull(Trigger.Interval(15).timezone)
    }

    @Test
    fun aTriggerThisAppDoesNotKnowStillDecodes() {
        // A newer server's kind must not break the list: it is kept as it came, and shown read-only.
        val list = CedarJson.decodeFromString(AgentList.serializer(), """
        {"agents":[{"id":"a1","title":"Tides","enabled":true,"trigger":{"kind":"tide","station":"Brest","above_m":4.5}},
                   {"id":"a2","title":"Odd clock","trigger":{"kind":"time","frequency":"daily"}},
                   {"id":"a3","title":"Odd source","trigger":{"kind":"condition","source":{"type":"sensor","pin":4},"predicate":{"op":"changes"}}},
                   {"id":"a4","title":"No trigger","trigger":null},
                   {"id":"a5","title":"Not an object","trigger":"soon"},
                   {"id":"a6","title":"News","trigger":{"kind":"interval","every_minutes":60}}]}
        """)
        assertEquals(6, list.agents.size)
        val tide = list.agents[0].trigger as Trigger.Unknown
        assertEquals("tide", tide.kind)
        assertEquals(4.5, tide.raw["above_m"].dbl)
        assertEquals(TriggerGroup.OTHER, tide.group)
        assertEquals(Agent.Status.Active, list.agents[0].status)
        assertNull(TriggerDraft.from(tide, "UTC"), "it can't be edited here")
        // One of ours that doesn't read as it should is kept the same way.
        assertEquals("time", (list.agents[1].trigger as Trigger.Unknown).kind)
        assertEquals(TriggerGroup.OTHER, list.agents[1].trigger.group)
        assertEquals("condition", (list.agents[2].trigger as Trigger.Unknown).kind)
        assertEquals(Trigger.Unknown(""), list.agents[3].trigger)
        assertEquals(Trigger.Unknown(""), list.agents[4].trigger)
        assertEquals(Trigger.Interval(60), list.agents[5].trigger)
        // And goes back out as it came, were it ever sent.
        assertEquals(j("""{"kind":"tide","station":"Brest","above_m":4.5}"""), tide.toJson())
    }

    @Test
    fun derivesWhatAnAgentIsDoing() {
        fun agent(json: String): Agent = CedarJson.decodeFromString(Agent.serializer(), """{"id":"a","title":"t",$json}""")
        val interval = """"trigger":{"kind":"interval","every_minutes":60}"""
        val once = """"trigger":{"kind":"once","at":"2026-12-03T09:00","timezone":"UTC"}"""
        val email = """"trigger":{"kind":"email","query":"","every_minutes":5}"""

        assertEquals(Agent.Status.Paused, agent(""""enabled":false,"next_run_at":"2026-12-03T09:00:00Z",$interval""").status)
        assertEquals(Agent.Status.Paused, agent(""""enabled":false,"next_run_at":"2026-12-03T09:00:00Z",$once""").status)
        // A one-off that has run switched itself off, and has no next run.
        assertEquals(Agent.Status.Done, agent(""""enabled":false,"next_run_at":null,"last_run_at":"2026-12-03T09:00:02Z",$once""").status)
        // Paused wins over a failing check.
        assertEquals(Agent.Status.Paused, agent(""""enabled":false,"last_error":"timeout",$email""").status)
        assertEquals(Agent.Status.CheckFailing("Gmail is not connected"), agent(""""enabled":true,"last_error":"Gmail is not connected",$email""").status)
        assertEquals(Agent.Status.NextRun(CedarDate.parse("2026-12-03T09:00:00Z")), agent(""""next_run_at":"2026-12-03T09:00:00Z",$once""").status)
        assertEquals(Agent.Status.NextRun(null), agent(interval).status)
        assertEquals(
            Agent.Status.Watching(CedarDate.parse("2026-11-12T10:00:00Z")),
            agent(""""next_check_at":"2026-11-12T10:05:00Z","last_checked_at":"2026-11-12T10:00:00Z",$email""").status,
        )
        assertEquals(Agent.Status.Watching(null), agent(email).status)
        assertEquals(Agent.Status.WaitingForCalls, agent(""""trigger":{"kind":"webhook"}""").status)
        assertEquals(Agent.Status.CallWaiting, agent(""""waiting":true,"trigger":{"kind":"webhook"}""").status)
    }

    @Test
    fun decodesTheActivityFeedAndATriedTrigger() {
        val page = CedarJson.decodeFromString(ActivityPage.serializer(), """
        {"items":[{"id":"r9","schedule_id":"a1","conversation_id":"c9","turn_id":"t9","due_at":"2026-11-12T08:00:00Z",
                   "started_at":"2026-11-12T08:00:01.5Z","manual":false,"error":null,
                   "cause":{"kind":"mail","summary":"New mail from Ada: Invoice","excerpt":"From: Ada","count":1},
                   "turn":{"id":"t9","conversation_id":"c9","seq":1,"run_id":"x","status":"running"},
                   "agent":{"id":"a1","title":"Invoices","trigger_kind":"email"}},
                  {"id":"r8","schedule_id":"a2","conversation_id":"c8","turn_id":"t8","due_at":"2026-11-12T07:00:00Z",
                   "started_at":"2026-11-12T07:00:00Z","manual":true,"error":null,"cause":null,"turn":null,
                   "agent":{"id":"a2","title":"News","trigger_kind":"tide"}}],
         "next_before":"2026-11-12T07:00:00.123456Z"}
        """)
        assertEquals(listOf("Invoices", "News"), page.items.map { it.agent?.title })
        assertEquals("email", page.items[0].agent?.triggerKind)
        assertEquals("New mail from Ada: Invoice", page.items[0].startedBy)
        assertTrue(page.items[0].isActive)
        assertTrue(page.items[1].manual)
        assertNull(page.items[1].startedBy)
        // Handed back as it came, so the next page starts exactly there.
        assertEquals("2026-11-12T07:00:00.123456Z", page.nextBefore)
        assertNull(CedarJson.decodeFromString(ActivityPage.serializer(), """{"items":[],"next_before":null}""").nextBefore)

        val fires = CedarJson.decodeFromString(TriggerTest.serializer(), """
        {"ok":true,"value":"38.5","would_fire":true,"summary":"The value is 38.5 (below 40)","sample":{"text":"{\n  \"price\": 38.5\n}","cut":false,"json":true}}
        """)
        assertTrue(fires.ok && fires.wouldFire)
        assertEquals("38.5", fires.valueText)
        assertEquals("The value is 38.5 (below 40)", fires.summary)
        assertEquals(true, fires.sample?.json)
        val waits = CedarJson.decodeFromString(TriggerTest.serializer(), """
        {"ok":true,"value":"3 threads","would_fire":false,"summary":null,"sample":{"text":"…","cut":true,"json":false}}
        """)
        assertFalse(waits.wouldFire)
        assertNull(waits.summary)
        assertEquals(true, waits.sample?.cut)
        val failed = CedarJson.decodeFromString(TriggerTest.serializer(), """{"ok":false,"error":"nothing at /price/usd","sample":{"text":"{}","cut":false,"json":true}}""")
        assertFalse(failed.ok)
        assertEquals("nothing at /price/usd", failed.error)
        assertEquals("{}", failed.sample?.text)
        assertNull(CedarJson.decodeFromString(TriggerTest.serializer(), """{"ok":false,"error":"HTTP 500"}""").sample)

        // A conversation that is an agent's run says whose.
        val conversation = CedarJson.decodeFromString(ConversationDetail.serializer(), """
        {"conversation":{"id":"c9","title":"Invoices · Thu 12 Nov","schedule_id":"a1","created_at":"2026-11-12T08:00:00Z","updated_at":"2026-11-12T08:00:00Z"},
         "turns":[],"agent":{"id":"a1","title":"Invoices","trigger_kind":"email"}}
        """)
        assertEquals(AgentRef("a1", "Invoices", "email"), conversation.agent)

        val catalog = CedarJson.decodeFromString(ToolCatalog.serializer(), """
        {"groups":[{"id":"weather","label":"Weather","available":true,"tools":[{"name":"weather","description":"The forecast","writes":false,"pollable":true}]},
                   {"id":"gmail","label":"Gmail","available":false,"reason":"Google is not connected","tools":[
                      {"name":"gmail_search_threads","description":"Search","writes":false,"pollable":true},{"name":"gmail_send","description":"Send","writes":true}]},
                   {"id":"custom","label":"Your tools","available":true,"tools":[{"name":"door_sensor","description":"","pollable":true}]},
                   {"id":"mcp_notes","available":true,"discovered":false,"tools":[]}],
         "sources":[{"name":"url","description":"A web address"}]}
        """)
        assertEquals(listOf("weather", "gmail_search_threads", "door_sensor"), catalog.pollable.map { it.second.name })
        assertTrue(catalog.isPersonal("door_sensor"))
        assertFalse(catalog.isPersonal("weather"))
        assertEquals("mcp_notes", catalog.groups[3].label)
        assertEquals(false, catalog.groups[3].discovered)

        val notifications = CedarJson.decodeFromString(NotificationStatus.serializer(), """{"default_method":"android","scope":"attention","configured":["android","email"],"desktop":{"listening":false}}""")
        assertEquals("android", notifications.defaultMethod)
        assertEquals(listOf("android", "email"), notifications.configured)
    }

    @Test
    fun encodesAnAgentForEveryKindOfTrigger() {
        fun body(trigger: Trigger) = AgentBody(title = "T", prompt = "P", trigger = trigger, timezone = "Europe/Zurich").toJson()
        fun expect(trigger: String, value: Trigger) =
            assertEquals(j("""{"title":"T","prompt":"P","trigger":$trigger,"timezone":"Europe/Zurich"}"""), body(value))

        expect("""{"kind":"time","frequency":"weekly","weekday":2,"time":"07:30","timezone":"Europe/Zurich"}""", Trigger.Time("weekly", 2, "07:30", "Europe/Zurich"))
        // No weekday unless it is weekly: the server refuses what it doesn't expect.
        expect("""{"kind":"time","frequency":"daily","time":"07:30","timezone":"Europe/Zurich"}""", Trigger.Time("daily", null, "07:30", "Europe/Zurich"))
        expect("""{"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Zurich"}""", Trigger.Once("2026-12-03T09:00", "Europe/Zurich"))
        expect("""{"kind":"interval","every_minutes":45}""", Trigger.Interval(45))
        expect("""{"kind":"monthly","day":1,"time":"08:00","timezone":"Europe/Zurich"}""", Trigger.Monthly(1, "08:00", "Europe/Zurich"))
        // A URL source's format is called `as`, and is left out when the server is to tell.
        expect(
            """{"kind":"condition","source":{"type":"url","url":"https://example.com/p","as":"json"},"extract":"price.eur","predicate":{"op":"lt","value":40},"every_minutes":10}""",
            Trigger.Condition(WatchSource.Url("https://example.com/p", "json"), "price.eur", Predicate.of("lt", "40")!!, 10),
        )
        expect(
            """{"kind":"condition","source":{"type":"url","url":"https://example.com/p"},"predicate":{"op":"changes"},"every_minutes":5}""",
            Trigger.Condition(WatchSource.Url("https://example.com/p"), null, Predicate.of("changes", "ignored")!!),
        )
        expect(
            """{"kind":"condition","source":{"type":"tool","name":"weather","input":{"place":"Bern"}},"extract":"/now/temp","predicate":{"op":"gte","value":30.5},"every_minutes":60}""",
            Trigger.Condition(WatchSource.Tool("weather", j("""{"place":"Bern"}""") as kotlinx.serialization.json.JsonObject), "/now/temp", Predicate.of("gte", "30,5")!!, 60),
        )
        expect(
            """{"kind":"condition","source":{"type":"records","set":"running_log"},"extract":"count","predicate":{"op":"contains","value":"12"},"every_minutes":5}""",
            Trigger.Condition(WatchSource.Records("running_log"), "count", Predicate.of("contains", " 12 ")!!),
        )
        expect("""{"kind":"webhook"}""", Trigger.Webhook)
        expect("""{"kind":"email","query":"is:unread","every_minutes":5}""", Trigger.Email("is:unread"))
        expect("""{"kind":"calendar","filter":"standup","minutes_before":15,"new_events":false,"every_minutes":5}""", Trigger.Calendar(filter = "standup", minutesBefore = 15))
        expect("""{"kind":"calendar","calendar_id":"work","new_events":true,"every_minutes":30}""", Trigger.Calendar(calendarId = "work", newEvents = true, everyMinutes = 30))

        assertNull(Predicate.of("gt", "warm"), "a threshold needs a number")
        assertNull(Predicate.of("equals", "  "), "equals needs something to compare with")

        // Everything a body can carry; `null` is told from absent.
        assertEquals(
            j("""{"title":"T","prompt":"P","model":"m1","reasoning":"high","enabled":false,"conversation_id":"c1","tools":["weather","web"],"notify":"none",
                  "trigger":{"kind":"webhook"},"timezone":"UTC","min_interval_secs":600,"max_runs_per_day":10}"""),
            AgentBody(
                title = "T", prompt = "P", model = "m1", reasoning = "high", enabled = false, conversationId = "c1",
                tools = Change.Set(listOf("weather", "web")), notify = Change.Set("none"), trigger = Trigger.Webhook, timezone = "UTC",
                minIntervalSecs = Change.Set(600), maxRunsPerDay = Change.Set(10),
            ).toJson(),
        )
        assertEquals(
            j("""{"model":"","reasoning":"","tools":null,"notify":null,"min_interval_secs":null,"max_runs_per_day":null}"""),
            AgentBody(model = "", reasoning = "", tools = Change.Clear, notify = Change.Clear, minIntervalSecs = Change.Clear, maxRunsPerDay = Change.Clear).toJson(),
        )
        assertEquals(j("""{"enabled":true}"""), AgentBody(enabled = true).toJson())
    }

    @Test
    fun buildsATriggerFromWhatTheFormHolds() {
        fun ready(draft: TriggerDraft, personalTool: Boolean = false): Trigger = (draft.build(personalTool = personalTool) as TriggerDraft.Built.Ready).trigger
        fun problem(draft: TriggerDraft, personalTool: Boolean = false, now: java.time.LocalDateTime? = null): TriggerProblem =
            (draft.build(personalTool = personalTool, now = now) as TriggerDraft.Built.Invalid).problem
        val draft = TriggerDraft(timezone = "Europe/Zurich")

        assertEquals(Trigger.Time("daily", null, "08:00", "Europe/Zurich"), ready(draft))
        assertEquals(Trigger.Time("weekly", 4, "07:05", "Europe/Zurich"), ready(draft.copy(frequency = "weekly", weekday = 4, time = "7:05")))
        assertEquals(TriggerProblem.Time, problem(draft.copy(time = "25:00")))
        assertEquals(TriggerProblem.Time, problem(draft.copy(time = "soon")))

        val once = draft.copy(kind = TriggerKind.ONCE, date = "2026-12-03", time = "09:00")
        assertEquals(Trigger.Once("2026-12-03T09:00", "Europe/Zurich"), ready(once))
        assertEquals(TriggerProblem.Date, problem(once.copy(date = "")))
        assertEquals(TriggerProblem.InThePast, problem(once, now = java.time.LocalDateTime.parse("2026-12-03T09:00:00")))
        assertEquals(Trigger.Once("2026-12-03T09:00", "Europe/Zurich"), (once.build(now = java.time.LocalDateTime.parse("2026-12-03T08:59:00")) as TriggerDraft.Built.Ready).trigger)

        val interval = draft.copy(kind = TriggerKind.INTERVAL)
        assertEquals(Trigger.Interval(60), ready(interval))
        assertEquals(Trigger.Interval(2880), ready(interval.copy(every = "2", everyUnit = Span.DAYS)))
        assertEquals(TriggerProblem.Every(15, 10080), problem(interval.copy(every = "10", everyUnit = Span.MINUTES)))
        assertEquals(TriggerProblem.Every(15, 10080), problem(interval.copy(every = "8", everyUnit = Span.DAYS)))
        assertEquals(TriggerProblem.Every(15, 10080), problem(interval.copy(every = "")))

        assertEquals(Trigger.Monthly(31, "08:00", "Europe/Zurich"), ready(draft.copy(kind = TriggerKind.MONTHLY, day = "31")))
        assertEquals(TriggerProblem.Day, problem(draft.copy(kind = TriggerKind.MONTHLY, day = "32")))

        val condition = draft.copy(kind = TriggerKind.CONDITION, url = " https://example.com/p ", urlFormat = "json", extract = " price ", op = "lt", value = "40")
        assertEquals(Trigger.Condition(WatchSource.Url("https://example.com/p", "json"), "price", Predicate("lt", j("40")), 5), ready(condition))
        assertEquals(WatchSource.Url("https://example.com/p", null), (ready(condition.copy(urlFormat = "")) as Trigger.Condition).source)
        assertEquals(TriggerProblem.Url, problem(condition.copy(url = "example.com")))
        assertEquals(TriggerProblem.Number, problem(condition.copy(value = "cheap")))
        assertEquals(TriggerProblem.Value, problem(condition.copy(op = "equals", value = "")))
        assertEquals(Predicate("changes"), (ready(condition.copy(op = "changes")) as Trigger.Condition).predicate)
        assertEquals(TriggerProblem.Every(1, 10080), problem(condition.copy(check = "0")))
        val tool = condition.copy(sourceType = TriggerDraft.SOURCE_TOOL, tool = "door_sensor", toolInput = "", check = "2")
        assertEquals(WatchSource.Tool("door_sensor"), (ready(tool) as Trigger.Condition).source)
        assertEquals(TriggerProblem.Tool, problem(tool.copy(tool = "")))
        assertEquals(TriggerProblem.ToolInput, problem(tool.copy(toolInput = "[1]")))
        assertEquals(TriggerProblem.ToolInput, problem(tool.copy(toolInput = "{place")))
        // A personal tool starts a container for every check, so it is checked less often.
        assertEquals(TriggerProblem.Every(5, 10080), problem(tool, personalTool = true))
        assertEquals(2, (ready(tool) as Trigger.Condition).everyMinutes)
        val records = condition.copy(sourceType = TriggerDraft.SOURCE_RECORDS, recordSet = "running_log", extract = "count", op = "gte", value = "10")
        assertEquals(Trigger.Condition(WatchSource.Records("running_log"), "count", Predicate("gte", j("10")), 5), ready(records))
        assertEquals(TriggerProblem.RecordSet, problem(records.copy(recordSet = " ")))

        assertEquals(Trigger.Webhook, ready(draft.copy(kind = TriggerKind.WEBHOOK)))
        assertEquals(Trigger.Email("is:unread", 120), ready(draft.copy(kind = TriggerKind.EMAIL, query = " is:unread ", check = "2", checkUnit = Span.HOURS)))
        assertEquals(Trigger.Email("", 5), ready(draft.copy(kind = TriggerKind.EMAIL)), "an empty query is any mail")

        val calendar = draft.copy(kind = TriggerKind.CALENDAR, filter = " standup ")
        assertEquals(Trigger.Calendar(null, "standup", 30, false, 5), ready(calendar))
        assertEquals(Trigger.Calendar(null, null, null, true, 5), ready(calendar.copy(filter = "", beforeEvents = false, newEvents = true)))
        assertEquals(TriggerProblem.CalendarNeedsOne, problem(calendar.copy(beforeEvents = false)))
        assertEquals(TriggerProblem.MinutesBefore(10080), problem(calendar.copy(minutesBefore = "20000")))

        // Editing: the draft of a trigger builds that trigger again, in the trigger's own zone.
        val triggers = listOf(
            Trigger.Time("weekly", 6, "07:30", "Asia/Tokyo"),
            Trigger.Time("weekdays", null, "18:00", "Asia/Tokyo"),
            Trigger.Once("2026-12-03T09:00", "Asia/Tokyo"),
            Trigger.Interval(90),
            Trigger.Interval(10080),
            Trigger.Monthly(15, "06:45", "Asia/Tokyo"),
            Trigger.Condition(WatchSource.Url("https://example.com/p", "text"), null, Predicate("contains", j("\"sold out\"")), 1440),
            Trigger.Condition(WatchSource.Tool("weather", j("""{"place":"Bern"}""") as kotlinx.serialization.json.JsonObject), "/now/temp", Predicate("gt", j("30")), 60),
            Trigger.Condition(WatchSource.Records("running_log"), "count", Predicate("changes"), 5),
            Trigger.Webhook,
            Trigger.Email("label:invoices", 15),
            Trigger.Calendar("work", "standup", 0, true, 5),
            Trigger.Calendar(null, null, null, true, 5),
        )
        for (trigger in triggers) {
            assertEquals(trigger, ready(TriggerDraft.from(trigger, "Europe/Zurich")!!), "$trigger")
        }
        assertEquals("Asia/Tokyo", TriggerDraft.from(triggers[0], "Europe/Zurich")!!.timezone)
        assertEquals("Europe/Zurich", TriggerDraft.from(Trigger.Webhook, "Europe/Zurich")!!.timezone)
        assertEquals(90 to Span.MINUTES, Span.split(90))
        assertEquals(2 to Span.HOURS, Span.split(120))
        assertEquals(7 to Span.DAYS, Span.split(10080))
    }

    @Test
    fun parsesDatesWithAnyPrecision() {
        assertTrue(CedarDate.parse("2026-10-12T03:36:00Z") != null)
        assertTrue(CedarDate.parse("2026-10-12T03:36:00.1Z") != null)
        assertTrue(CedarDate.parse("2026-10-12T03:36:00.123456789+00:00") != null)
        assertNull(CedarDate.parse("yesterday"))
    }

    @Test
    fun errorEnvelope() {
        val body = """{"error":{"code":"conflict","message":"wait for the current response to finish, or stop it"}}""".toByteArray()
        assertEquals(
            ChatException.Api(409, "conflict", "wait for the current response to finish, or stop it"),
            ChatClient.error(409, body),
        )
        assertEquals(ChatException.Unauthorized, ChatClient.error(401, ByteArray(0)))
        assertEquals(ChatException.Api(502, "http_502", "cedar-chat returned HTTP 502"), ChatClient.error(502, "<html>".toByteArray()))
    }

    @Test
    fun tlsErrorOnExplicitPortHintsAtTailscaleServe() {
        val tls = SSLHandshakeException("handshake failed")
        val hinted = ChatClient.unreachable(tls, "https://spark.ts.net:3000".toHttpUrl()) as ChatException.Unreachable
        assertTrue(hinted.detail.contains("Port 3000 may serve plain HTTP"))
        assertEquals(ChatException.Unreachable("handshake failed"), ChatClient.unreachable(tls, "https://spark.ts.net".toHttpUrl()))
        val offline = IOException("offline")
        assertEquals(ChatException.Unreachable("offline"), ChatClient.unreachable(offline, "https://spark.ts.net:3000".toHttpUrl()))
        // Android's network security config refuses cleartext: the error says to use HTTPS.
        val cleartext = UnknownServiceException("CLEARTEXT communication to chat.local not permitted by network security policy")
        assertEquals(
            ChatException.Unreachable(ServerSettings.HTTPS_ONLY, ChatException.Unreachable.Hint.HttpsOnly),
            ChatClient.unreachable(cleartext, "http://chat.local".toHttpUrl()),
        )
        assertEquals(ChatException.Unreachable.Hint.PlainHttpPort(3000, "handshake failed"), hinted.hint, "the app words the hint itself")
    }

    @Test
    fun webURLAndFileRequests() {
        val client = ChatClient("http://chat.local:3000/".toHttpUrl(), "tok")
        assertEquals("http://chat.local:3000/#/c/c1", client.webURL("c1"))
        val request = client.fileRequest("c1", "out/my chart.png")
        assertEquals("http://chat.local:3000/api/conversations/c1/files/out/my%20chart.png", request.url.toString())
        assertEquals("Bearer tok", request.header("Authorization"))
        val open = ChatClient.from(ServerSettings(serverURL = "http://x"))
        assertNull(open.fileRequest("c", "a").header("Authorization"))
        assertFailsWith<ChatException.InvalidURL> { ChatClient.from(ServerSettings(serverURL = "ftp://x")) }
        assertFailsWith<ChatException.NotConfigured> { ChatClient.from(ServerSettings(serverURL = " ")) }
    }

    @Test
    fun httpsOnly() {
        assertEquals(ServerSettings.HTTPS_ONLY, ServerSettings(serverURL = "http://127.0.0.1:3000").urlProblem())
        assertNull(ServerSettings(serverURL = "https://m.t.ts.net").urlProblem())
        assertEquals(ChatException.InvalidURL.message, ServerSettings(serverURL = "m.t.ts.net").urlProblem())
        assertEquals(ChatException.NotConfigured.message, ServerSettings().urlProblem())
        assertEquals(ServerSettings.UrlProblem.NOT_HTTPS, ServerSettings(serverURL = "http://127.0.0.1:3000").urlIssue())
        assertEquals(ServerSettings.UrlProblem.INVALID, ServerSettings(serverURL = "m.t.ts.net").urlIssue())
        assertEquals(ServerSettings.UrlProblem.NOT_CONFIGURED, ServerSettings().urlIssue())
        assertTrue(ServerSettings("https://a", "t").sameServer(ServerSettings(" https://a ", "t ", model = "m")))
        assertFalse(ServerSettings("https://a", "t").sameServer(ServerSettings("https://a", "u")))
    }

    @Test
    fun saysWhenTheServerIsReachedThroughTailscale() {
        assertTrue(ServerSettings.isTailnetHost("m.tail1234.ts.net"))
        assertTrue(ServerSettings.isTailnetHost("M.Tail1234.TS.NET."))
        assertTrue(ServerSettings.isTailnetHost("100.64.0.1"))
        assertTrue(ServerSettings.isTailnetHost("100.127.255.254"))
        assertTrue(ServerSettings.isTailnetHost("[fd7a:115c:a1e0::1]"))
        assertFalse(ServerSettings.isTailnetHost("100.128.0.1"))
        assertFalse(ServerSettings.isTailnetHost("100.63.0.1"))
        assertFalse(ServerSettings.isTailnetHost("192.168.1.10"))
        assertFalse(ServerSettings.isTailnetHost("chat.example.com"))
        assertFalse(ServerSettings.isTailnetHost("ts.net.example.com"))

        assertTrue(ServerSettings(serverURL = "https://m.t.ts.net").needsTailscale)
        assertTrue(ServerSettings(serverURL = "https://100.101.102.103").needsTailscale)
        assertTrue(ServerSettings(serverURL = "https://[fd7a:115c:a1e0::1]").needsTailscale)
        // Nothing usable yet, or the phone's own address: the usual setup is tailscale serve.
        assertTrue(ServerSettings().needsTailscale)
        assertTrue(ServerSettings(serverURL = "http://127.0.0.1:3000").needsTailscale)
        assertTrue(ServerSettings(serverURL = "m.t.ts.net").needsTailscale)
        // A public HTTPS server (Caddy, Let's Encrypt) needs no Tailscale.
        assertFalse(ServerSettings(serverURL = "https://chat.example.com").needsTailscale)
        assertFalse(ServerSettings(serverURL = "https://192.168.1.10").needsTailscale)
    }

    @Test
    fun decodesAContributionPreviewAndRebuildsTheBundleToSend() {
        val json = """
        {"bundle":{"schema_version":1,"model":{"id":"m","name":"org/model-a"},"redactions":{"EMAIL":["maya.lindqvist@example.net"],"USER":["theo"]},
          "dropped":{"attachments":1,"memory":true,"fetched_bodies":0,"reasoning":1},
          "turns":[{"seq":1,"prompt":"list my files, I am maya.lindqvist@example.net","status":"completed","day":"2026-10-31","rating":1,
            "messages":[{"role":"user","content":[{"type":"text","text":"list my files"}]},
                        {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"bash","input":{"command":"ls /Users/theo"}}]},
                        {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"a.txt"}]},
                        {"role":"assistant","content":[{"type":"text","text":"One file."}]}]}]},
         "blocked":[],"secrets":[],"problems":[],"can_send":true,"accepted":false,"private_tools":[],"turn_ids":["turn-1"]}
        """
        val p = CedarJson.decodeFromString(ContributionPreview.serializer(), json)
        assertTrue(p.canSend)
        assertFalse(p.accepted)
        assertEquals("org/model-a", p.modelName)
        assertEquals(2, p.standInCount)
        assertEquals(listOf("maya.lindqvist@example.net"), p.standIns["EMAIL"])
        val turns = p.turns()
        assertEquals(1, turns.size)
        assertEquals(1, turns[0].rating)
        assertEquals(listOf("One file."), turns[0].agentText)
        assertEquals("bash", turns[0].toolCalls[0].first)
        val sent = p.bundleWith("partly", " it missed b.txt ", mapOf(1 to -1))
        assertEquals("partly", sent["outcome"]["result"].str)
        assertEquals("it missed b.txt", sent["outcome"]["notes"].str)
        assertEquals(-1, sent["turns"].arr!![0]["rating"].int)
        assertEquals(1, sent["schema_version"].int)
        val cleared = p.bundleWith(null, null, mapOf(1 to null))
        assertNull(cleared["outcome"])
        assertNull(cleared["turns"].arr!![0]["rating"])
    }

    @Test
    fun decodesContributionSettingsAndReceipts() {
        val s = CedarJson.decodeFromString(
            ContributionSettings.serializer(),
            """{"license":"cc-by-4.0","pseudonym":"p_0123456789abcdef","include_reasoning":false,"include_fetched":true,
                "accepted_at":"2026-10-31T10:00:00Z","available":true,"host":"contrib.example.org","licenses":["cc0","cc-by-4.0"]}""",
        )
        assertTrue(s.enabled)
        assertEquals("contrib.example.org", s.host)
        val off = CedarJson.decodeFromString(ContributionSettings.serializer(), """{"license":"cc0","available":false,"licenses":[]}""")
        assertFalse(off.enabled)
        assertNull(off.host)
        val list = CedarJson.decodeFromString(
            ContributionList.serializer(),
            """{"contributions":[{"id":"r-1","conversation_id":"c1","title":"Files","content_sha256":"ab","license":"cc0","url":"https://x",
                "submitted_at":"2026-10-31T10:00:00Z","withdrawn_at":null,"status":"released","released_in":["v2026.11"],"checked_at":"2026-11-03T00:00:00Z"}]}""",
        )
        assertEquals("released", list.contributions[0].status)
        assertEquals(listOf("v2026.11"), list.contributions[0].releasedIn)
        assertNull(list.contributions[0].withdrawnAt)
        val cfg = CedarJson.decodeFromString(ServerConfig.serializer(), """{"auth_required":false,"admin_enabled":false,"memory":true,"contributions":true}""")
        assertTrue(cfg.contributions)
    }
}
