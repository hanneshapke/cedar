package dev.cedar.core

import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

/** The client against a fake cedar-chat: the requests it makes and how it reads the answers. */
class ChatClientTest {
    private lateinit var server: MockWebServer
    private lateinit var client: ChatClient

    @BeforeTest
    fun start() {
        server = MockWebServer()
        server.start()
        client = ChatClient(server.url("/"), "tok")
    }

    @AfterTest
    fun stop() = server.shutdown()

    private fun json(body: String, code: Int = 200) =
        MockResponse().setResponseCode(code).setHeader("Content-Type", "application/json").setBody(body)

    @Test
    fun sendsTheTokenAndReadsTheList() = runBlocking {
        server.enqueue(json("""{"conversations":[{"id":"c1","title":"Hi","unread":true,"needs_answer":false,
            "created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:36:00Z"}]}"""))
        val list = client.conversations()
        assertEquals(listOf("c1"), list.map { it.id })
        val request = server.takeRequest()
        assertEquals("/api/conversations", request.path)
        assertEquals("Bearer tok", request.getHeader("Authorization"))
    }

    @Test
    fun postsAMessageWithAttachments() = runBlocking {
        server.enqueue(json("""{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"look","status":"queued"}"""))
        val turn = client.send("c1", ChatClient.SendMessage("look", model = "m", attachments = listOf(ChatClient.SendMessage.Upload("a.pdf", "application/pdf", "JVBERg=="))))
        assertEquals(TurnStatus.QUEUED, turn.status)
        val request = server.takeRequest()
        assertEquals("POST", request.method)
        assertEquals("/api/conversations/c1/messages", request.path)
        assertEquals(
            j("""{"content":"look","model":"m","attachments":[{"name":"a.pdf","media_type":"application/pdf","data":"JVBERg=="}]}"""),
            j(request.body.readUtf8()),
        )
    }

    @Test
    fun postsTheLocaleWithAMessage() = runBlocking {
        server.enqueue(json("""{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"hallo","status":"queued"}"""))
        val locale = AppLocales.effective(null, java.util.Locale.forLanguageTag("de-CH"))
        client.send("c1", ChatClient.SendMessage("hallo", locale = locale))
        assertEquals(j("""{"content":"hallo","locale":"de-CH"}"""), j(server.takeRequest().body.readUtf8()))
    }

    @Test
    fun setsAndClearsTheServersLocale() = runBlocking {
        server.enqueue(json("""{"locale":"fr-BE"}"""))
        server.enqueue(json("""{"locale":null}"""))
        client.setLocale("fr-BE")
        client.setLocale(null)
        server.takeRequest().let {
            assertEquals("PUT /api/settings/locale", "${it.method} ${it.path}")
            assertEquals(j("""{"locale":"fr-BE"}"""), j(it.body.readUtf8()))
        }
        server.takeRequest().let {
            assertEquals("PUT /api/settings/locale", "${it.method} ${it.path}")
            assertEquals(j("""{"locale":null}"""), j(it.body.readUtf8()), "Automatic clears it, explicitly")
        }
    }

    @Test
    fun searchesTheConversations() = runBlocking {
        server.enqueue(json("""{"conversations":[{"id":"c1","title":"C++ notes","snippet":"…templates & more…",
            "created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:36:00Z"}]}"""))

        val found = client.search("c++ & more")

        assertEquals("GET /api/conversations/search?q=c%2B%2B%20%26%20more", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals(listOf("c1"), found.map { it.id })
        assertEquals("…templates & more…", found[0].snippet)
    }

    @Test
    fun starsAndRegistersThePhone() = runBlocking {
        val conversation = """{"id":"c1","unread":false,"needs_answer":false,"starred_at":"2026-10-23T08:00:00Z",
            "created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:36:00Z"}"""
        server.enqueue(json(conversation))
        server.enqueue(json(conversation))
        server.enqueue(json("""{"device":{"id":"d1","transport":"unifiedpush","name":"Pixel"},"device_id":"d1"}"""))
        server.enqueue(json("{}"))

        client.setStarred("c1", true)
        client.setStarred("c1", false)
        val device = client.registerAndroidDevice("unifiedpush", "https://ntfy.sh/upAbc", "Pixel")
        client.unregisterAndroidDevice("https://ntfy.sh/upAbc")

        assertEquals("PUT /api/conversations/c1/star", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/conversations/c1/star", server.takeRequest().let { "${it.method} ${it.path}" })
        server.takeRequest().let {
            assertEquals("POST /api/notifications/android/devices", "${it.method} ${it.path}")
            assertEquals(j("""{"transport":"unifiedpush","token":"https://ntfy.sh/upAbc","name":"Pixel"}"""), j(it.body.readUtf8()))
        }
        assertEquals("d1", device?.id)
        server.takeRequest().let {
            assertEquals("DELETE /api/notifications/android/devices", "${it.method} ${it.path}")
            assertEquals(j("""{"token":"https://ntfy.sh/upAbc"}"""), j(it.body.readUtf8()))
        }
    }

    @Test
    fun readsAndForgetsMemory() = runBlocking {
        server.enqueue(json("""{"supported":true,"limits":{"max_items":200,"max_chars":500},
            "profile":{"text":"Ana, in Zurich.","user_edited":false,"updated_at":"2026-11-06T08:00:00Z"},"items":[
            {"id":"m1","kind":"preference","text":"Prefers metric units","support":"I always want metric units","pinned":true,
             "source":"user","recall_count":3,"run_id":"r1","conversation_id":"c1","title":"Units",
             "valid_from":"2026-10-29T08:00:00Z","valid_until":null,"archived_at":null,"superseded_by":null,
             "created_at":"2026-10-29T08:00:00Z","updated_at":"2026-10-29T08:00:00Z"},
            {"id":"m2","text":"Has a cat","run_id":null,"conversation_id":null,"title":null,
             "created_at":"2026-10-29T09:00:00Z","updated_at":"2026-10-29T09:00:00Z"}]}"""))
        server.enqueue(MockResponse().setResponseCode(204))
        server.enqueue(MockResponse().setResponseCode(204))
        // A chat server from before memory.
        server.enqueue(json("""{"error":{"code":"not_found","message":"not found"}}""", 404))

        // Archived rows, a pin, and the profile.
        server.enqueue(json("""{"supported":true,"items":[],"profile":null,"limits":null}"""))
        server.enqueue(json("""{"id":"m2","text":"Has a cat","pinned":true,"created_at":"2026-10-29T09:00:00Z","updated_at":"2026-10-29T09:00:00Z"}"""))
        server.enqueue(json("""{"profile":{"text":"Ana.","user_edited":true,"updated_at":"2026-11-06T09:00:00Z"}}"""))

        val list = client.memory()
        assertEquals(true, list.supported)
        assertEquals(200, list.limits?.maxItems)
        assertEquals("Ana, in Zurich.", list.profile?.text)
        assertEquals(listOf("m1", "m2"), list.items.map { it.id })
        assertEquals("c1", list.items[0].conversationId)
        assertEquals("Units", list.items[0].title)
        assertEquals("preference", list.items[0].kind)
        assertEquals("I always want metric units", list.items[0].support)
        assertEquals(true, list.items[0].pinned)
        assertEquals(true, list.items[0].isActive)
        // A cedar from before kinds: a fact, not pinned, with no quote.
        assertEquals(null, list.items[1].conversationId)
        assertEquals("fact", list.items[1].kind)
        assertEquals(false, list.items[1].pinned)
        client.deleteMemory("m1")
        client.deleteAllMemory()
        assertEquals(MemoryList(supported = false), client.memory())
        assertEquals(emptyList<MemoryItem>(), client.memory(MemoryFilter.ARCHIVE).items)
        assertEquals(true, client.updateMemory("m2", pinned = true).pinned)
        assertEquals(true, client.setMemoryProfile("Ana.")?.userEdited)

        assertEquals("GET /api/memory", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/memory/m1", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/memory", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/memory", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/memory?archived=true", server.takeRequest().let { "${it.method} ${it.path}" })
        server.takeRequest().let {
            assertEquals("PATCH /api/memory/m2", "${it.method} ${it.path}")
            assertEquals(j("""{"pinned":true}"""), j(it.body.readUtf8()))
        }
        server.takeRequest().let {
            assertEquals("PUT /api/memory/profile", "${it.method} ${it.path}")
            assertEquals(j("""{"text":"Ana."}"""), j(it.body.readUtf8()))
        }
    }

    @Test
    fun countsMemoriesOfEveryKind() = runBlocking {
        server.enqueue(json("""{"supported":true,"items":[
            {"id":"m1","text":"Prefers metric","kind":"preference","created_at":"2026-10-29T08:00:00Z","updated_at":"2026-10-29T08:00:00Z"},
            {"id":"m2","text":"Planned a trip","kind":"episode","created_at":"2026-10-29T09:00:00Z","updated_at":"2026-10-29T09:00:00Z"}],
            "profile":null,"limits":null}"""))
        server.enqueue(json("""{"supported":false,"items":[],"profile":null,"limits":null}"""))
        server.enqueue(json("""{"error":{"code":"not_found","message":"not found"}}""", 404))

        assertEquals(2, client.memoryCount())
        assertEquals(null, client.memoryCount())
        assertEquals(null, client.memoryCount())

        server.takeRequest().let {
            assertEquals("GET", it.method)
            assertEquals("/api/memory", it.requestUrl?.encodedPath)
            assertEquals(MemoryFilter.ALL_KINDS, it.requestUrl?.queryParameter("kind"))
        }
    }

    @Test
    fun readsAndDeletesRecords() = runBlocking {
        server.enqueue(json("""{"supported":true,"limits":{"max_sets":50,"max_items":10000},"sets":[
            {"id":"s1","name":"running_log","description":"Weekly runs","key_fields":["date"],"health_data":false,"count":1,
             "columns":["date","km"],"created_at":"2026-10-29T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"}]}"""))
        server.enqueue(json("""{"set":{"id":"s1","name":"running_log","description":"Weekly runs","key_fields":["date"],"health_data":false,"count":1,
             "columns":["date","km"],"created_at":"2026-10-29T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"},
             "records":[{"id":"r1","key":"2026-11-02","data":{"date":"2026-11-02","km":35.5},"run_id":"run-1","conversation_id":"c1","title":"Week 44",
             "created_at":"2026-11-02T08:00:00Z","updated_at":"2026-11-05T08:00:00Z"}]}"""))
        server.enqueue(MockResponse().setResponseCode(204))
        server.enqueue(MockResponse().setResponseCode(204))
        // A chat server from before records.
        server.enqueue(json("""{"error":{"code":"not_found","message":"not found"}}""", 404))

        val list = client.records()
        assertEquals(true, list.supported)
        assertEquals(50, list.limits?.maxSets)
        assertEquals(listOf("running_log"), list.sets.map { it.name })
        val detail = client.recordSet("running_log")
        assertEquals("c1", detail.records[0].conversationId)
        assertEquals("2026-11-02", detail.records[0].data["date"].str)
        client.deleteRecordSet("running_log")
        client.deleteAllRecords()
        assertEquals(RecordsList(supported = false), client.records())

        assertEquals("GET /api/records", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/records/running_log", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/records/running_log", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/records", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/records", server.takeRequest().let { "${it.method} ${it.path}" })
    }

    @Test
    fun listsMakesChangesAndDeletesAgents() = runBlocking {
        val agent = """{"id":"a1","title":"Door","prompt":"Say who came","enabled":true,"trigger":{"kind":"webhook"},
            "hook":{"url":"https://cedar.example/api/hooks/ab","path":"/api/hooks/ab","secret_last4":"cdef"}}"""
        val withSecret = """{"id":"a1","title":"Door","enabled":true,"trigger":{"kind":"webhook"},
            "hook":{"url":"https://cedar.example/api/hooks/ab","path":"/api/hooks/ab","secret_last4":"cdef","secret":"0123cdef"}}"""
        server.enqueue(json("""{"agents":[$agent],"limits":{"max_agents":100,"min_interval_secs":300,"max_runs_per_day":24,
            "min_interval_minutes":15,"check_minutes":5,"personal_check_minutes":5}}"""))
        server.enqueue(json("""{"agent":$agent,"runs":[{"id":"r1","schedule_id":"a1","conversation_id":"c1","turn_id":"t1",
            "due_at":"2026-11-12T08:00:00Z","started_at":"2026-11-12T08:00:00Z","manual":false,"error":null,
            "cause":{"kind":"webhook","summary":"A webhook call"},"turn":null}]}"""))
        server.enqueue(json(withSecret, 201))
        server.enqueue(json(agent))
        server.enqueue(json(agent))
        server.enqueue(json("""{"id":"r2","schedule_id":"a1","conversation_id":"c2","turn_id":"t2","due_at":"2026-11-12T09:00:00Z",
            "started_at":"2026-11-12T09:00:00Z","manual":true,"error":null,"cause":null,"turn":null}""", 202))
        server.enqueue(json(withSecret))
        server.enqueue(MockResponse().setResponseCode(204))

        val list = client.agents()
        assertEquals(listOf("Door"), list.agents.map { it.title })
        assertEquals(100, list.limits.maxAgents)
        val detail = client.agent("a1")
        assertEquals("A webhook call", detail.runs.single().startedBy)
        val made = client.createAgent(AgentBody(title = "Door", prompt = "Say who came", trigger = Trigger.Webhook, timezone = "Europe/Zurich", conversationId = "c0"))
        assertEquals("0123cdef", made.hook?.secret)
        client.updateAgent("a1", AgentBody(enabled = false))
        client.updateAgent(
            "a1",
            AgentBody(
                title = "Door", prompt = "p", model = "", reasoning = "", tools = Change.Clear, notify = Change.Set("none"),
                trigger = Trigger.Email("is:unread", 10), timezone = "Europe/Zurich", minIntervalSecs = Change.Set(600), maxRunsPerDay = Change.Clear,
            ),
        )
        val run = client.runAgent("a1")
        assertEquals("c2", run.conversationId)
        assertEquals("0123cdef", client.rotateHook("a1").hook?.secret)
        client.deleteAgent("a1")

        assertEquals("GET /api/agents", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/agents/a1", server.takeRequest().let { "${it.method} ${it.path}" })
        server.takeRequest().let {
            assertEquals("POST /api/agents", "${it.method} ${it.path}")
            assertEquals("Bearer tok", it.getHeader("Authorization"))
            assertEquals(
                j("""{"title":"Door","prompt":"Say who came","conversation_id":"c0","trigger":{"kind":"webhook"},"timezone":"Europe/Zurich"}"""),
                j(it.body.readUtf8()),
            )
        }
        server.takeRequest().let {
            assertEquals("PATCH /api/agents/a1", "${it.method} ${it.path}")
            assertEquals(j("""{"enabled":false}"""), j(it.body.readUtf8()))
        }
        server.takeRequest().let {
            assertEquals("PATCH /api/agents/a1", "${it.method} ${it.path}")
            assertEquals(
                j("""{"title":"Door","prompt":"p","model":"","reasoning":"","tools":null,"notify":"none",
                      "trigger":{"kind":"email","query":"is:unread","every_minutes":10},"timezone":"Europe/Zurich",
                      "min_interval_secs":600,"max_runs_per_day":null}"""),
                j(it.body.readUtf8()),
            )
        }
        assertEquals("POST /api/agents/a1/run", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("POST /api/agents/a1/hook/rotate", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/agents/a1", server.takeRequest().let { "${it.method} ${it.path}" })

        // Deleting while a run is going: the server's words reach the user.
        server.enqueue(json("""{"error":{"code":"conflict","message":"wait for its current run to finish, or stop it"}}""", 409))
        val e = assertFailsWith<ChatException.Api> { client.deleteAgent("a1") }
        assertEquals(409, e.status)
        assertEquals("wait for its current run to finish, or stop it", e.message)
    }

    @Test
    fun triesATriggerAndPagesTheActivity() = runBlocking {
        server.enqueue(json("""{"ok":true,"value":"38.5","would_fire":true,"summary":"Below 40","sample":{"text":"{}","cut":false,"json":true}}"""))
        server.enqueue(json("""{"error":{"code":"bad_request","message":"only a trigger that watches something can be tried out"}}""", 400))
        server.enqueue(json("""{"items":[{"id":"r2","schedule_id":"a1","agent":{"id":"a1","title":"News","trigger_kind":"time"}}],"next_before":"2026-11-12T07:00:00.123456Z"}"""))
        server.enqueue(json("""{"items":[],"next_before":null}"""))

        val trigger = Trigger.Condition(WatchSource.Url("https://example.com/p", "json"), "price", Predicate.of("lt", "40")!!, 10)
        val tried = client.testTrigger(trigger)
        assertEquals(true, tried.wouldFire)
        assertEquals("only a trigger that watches something can be tried out", assertFailsWith<ChatException.Api> { client.testTrigger(Trigger.Webhook) }.message)
        val first = client.activity()
        assertEquals("News", first.items.single().agent?.title)
        val second = client.activity(before = first.nextBefore, limit = 50)
        assertEquals(null, second.nextBefore)

        server.takeRequest().let {
            assertEquals("POST /api/agents/test", "${it.method} ${it.path}")
            assertEquals(
                j("""{"trigger":{"kind":"condition","source":{"type":"url","url":"https://example.com/p","as":"json"},"extract":"price",
                      "predicate":{"op":"lt","value":40},"every_minutes":10}}"""),
                j(it.body.readUtf8()),
            )
        }
        assertEquals(j("""{"trigger":{"kind":"webhook"}}"""), j(server.takeRequest().body.readUtf8()))
        assertEquals("GET /api/agents/activity", server.takeRequest().let { "${it.method} ${it.path}" })
        server.takeRequest().let {
            assertEquals("GET", it.method)
            assertEquals("/api/agents/activity", it.requestUrl?.encodedPath)
            assertEquals("2026-11-12T07:00:00.123456Z", it.requestUrl?.queryParameter("before"))
            assertEquals("50", it.requestUrl?.queryParameter("limit"))
        }
    }

    @Test
    fun asksForARunFromTheDevice() = runBlocking {
        server.enqueue(json("""{"started":true,"conversation_id":"c7"}""", 202))
        server.enqueue(json("""{"started":false,"held":"too_soon"}""", 202))
        server.enqueue(json("""{"error":{"code":"conflict","message":"this agent is paused"}}""", 409))
        server.enqueue(json("""{"error":{"code":"payload_too_large","message":"at most 16384 bytes can be handed to a run"}}""", 413))

        val started = client.triggerAgent("a1", source = "share", text = "Worth a look", title = "An article", url = "https://example.com/a")
        assertEquals(true, started.started)
        assertEquals("c7", started.conversationId)
        val held = client.triggerAgent("a1", source = "automation")
        assertEquals(false, held.started)
        assertEquals(DeviceStart.TOO_SOON, held.held)
        val paused = assertFailsWith<ChatException.Api> { client.triggerAgent("a2", source = "automation", text = "door open") }
        assertEquals(409, paused.status)
        assertEquals("this agent is paused", paused.message)
        assertEquals(413, assertFailsWith<ChatException.Api> { client.triggerAgent("a1", text = "x".repeat(20_000)) }.status)

        server.takeRequest().let {
            assertEquals("POST /api/agents/a1/trigger", "${it.method} ${it.path}")
            assertEquals("Bearer tok", it.getHeader("Authorization"))
            assertEquals(j("""{"source":"share","text":"Worth a look","title":"An article","url":"https://example.com/a"}"""), j(it.body.readUtf8()))
        }
        server.takeRequest().let {
            assertEquals("POST /api/agents/a1/trigger", "${it.method} ${it.path}")
            // Only what there is: the server refuses fields it doesn't know, and takes none at all.
            assertEquals(j("""{"source":"automation"}"""), j(it.body.readUtf8()))
        }
        server.takeRequest().let {
            assertEquals("POST /api/agents/a2/trigger", "${it.method} ${it.path}")
            assertEquals(j("""{"source":"automation","text":"door open"}"""), j(it.body.readUtf8()))
        }
        assertEquals("POST /api/agents/a1/trigger", server.takeRequest().let { "${it.method} ${it.path}" })
    }

    @Test
    fun decidesAboutASuggestedFollowUp() = runBlocking {
        server.enqueue(json("""{"followup":{"turn_id":"t1","call_id":"call_7","status":"accepted","agent_id":"a9","decided_at":"2026-11-12T10:00:00Z"},
            "agent":{"id":"a9","title":"Has the package shipped?","trigger":{"kind":"once","at":"2026-12-04T09:00","timezone":"Europe/Zurich"}}}"""))
        server.enqueue(json("""{"followup":{"turn_id":"t1","call_id":"call 8","status":"dismissed","agent_id":null,"decided_at":"2026-11-12T10:01:00Z"},"agent":null}"""))
        server.enqueue(json("""{"error":{"code":"bad_request","message":"at must be in the future"}}""", 400))
        server.enqueue(json("""{"conversation":{"id":"c1","created_at":"2026-11-12T09:00:00Z","updated_at":"2026-11-12T09:05:00Z"},"turns":[],
            "followups":[{"turn_id":"t1","call_id":"call_7","status":"accepted","agent_id":"a9","decided_at":"2026-11-12T10:00:00Z"}]}"""))

        val accept = FollowupChoice.Accept("Has the package shipped?", "Check the tracking page.", "2026-12-04T08:00:00Z", "Europe/Zurich")
        val accepted = client.decideFollowup("t1", "call_7", accept)
        assertEquals(true, accepted.followup.isAccepted)
        assertEquals("a9", accepted.agent?.id)
        val dismissed = client.decideFollowup("t1", "call 8", FollowupChoice.Dismiss)
        assertEquals(true, dismissed.followup.isDismissed)
        assertEquals(null, dismissed.agent)
        val late = assertFailsWith<ChatException.Api> { client.decideFollowup("t1", "call_9", accept) }
        assertEquals(400, late.status)
        assertEquals("at must be in the future", late.message)
        assertEquals(listOf("call_7"), client.conversation("c1").followups.map { it.callId })

        server.takeRequest().let {
            assertEquals("PUT /api/turns/t1/followups/call_7", "${it.method} ${it.path}")
            assertEquals("Bearer tok", it.getHeader("Authorization"))
            assertEquals(
                j("""{"decision":"accept","title":"Has the package shipped?","prompt":"Check the tracking page.","at":"2026-12-04T08:00:00Z","timezone":"Europe/Zurich"}"""),
                j(it.body.readUtf8()),
            )
        }
        server.takeRequest().let {
            // The call's id is one path component, whatever is in it.
            assertEquals("PUT /api/turns/t1/followups/call%208", "${it.method} ${it.path}")
            assertEquals(j("""{"decision":"dismiss"}"""), j(it.body.readUtf8()))
        }
        assertEquals("PUT /api/turns/t1/followups/call_9", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/conversations/c1", server.takeRequest().let { "${it.method} ${it.path}" })
    }

    @Test
    fun readsWhatTheAgentFormNeeds() = runBlocking {
        server.enqueue(json("""{"groups":[{"id":"weather","label":"Weather","available":true,"tools":[{"name":"weather","description":"","writes":false,"pollable":true}]}],
            "sources":[{"name":"url","description":"A web address"}]}"""))
        server.enqueue(json("""{"default_method":"android","scope":"all","configured":["android"],"desktop":{"listening":false}}"""))
        server.enqueue(json("""{"prompt":"Summarize today's news about cedar trees.","source":"run"}"""))
        server.enqueue(json("""{"error":{"code":"not_found","message":"the conversation has no finished response to schedule yet"}}""", 404))

        assertEquals(listOf("weather"), client.tools().pollable.map { it.second.name })
        assertEquals(listOf("android"), client.notifications().configured)
        assertEquals("Summarize today's news about cedar trees.", client.compact("c1").prompt)
        assertEquals(404, assertFailsWith<ChatException.Api> { client.compact("c2") }.status)

        assertEquals("GET /api/tools", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("GET /api/notifications", server.takeRequest().let { "${it.method} ${it.path}" })
        server.takeRequest().let {
            assertEquals("POST /api/conversations/c1/compact", "${it.method} ${it.path}")
            assertEquals("Bearer tok", it.getHeader("Authorization"))
        }
        assertEquals("POST /api/conversations/c2/compact", server.takeRequest().let { "${it.method} ${it.path}" })
    }

    @Test
    fun errorsCarryTheServersMessage() {
        server.enqueue(json("""{"error":{"code":"conflict","message":"wait for the current response to finish, or stop it"}}""", 409))
        val e = assertFailsWith<ChatException.Api> { runBlocking { client.cancel("t1") } }
        assertEquals(409, e.status)
        assertEquals("wait for the current response to finish, or stop it", e.message)

        server.enqueue(json("{}", 401))
        assertFailsWith<ChatException.Unauthorized> { runBlocking { client.inbox() } }

        server.enqueue(json("not json"))
        assertFailsWith<ChatException.BadResponse> { runBlocking { client.inbox() } }
    }

    @Test
    fun streamsATurnUntilItsTerminalEvent() = runBlocking {
        val body = buildString {
            append(": keep-alive\n\n")
            append("id: 3\nevent: run.started\ndata: {\"seq\":3,\"type\":\"run.started\",\"data\":{}}\n\n")
            append("data: not json\n\n")
            append("id: 4\ndata: {\"seq\":4,\"type\":\"assistant.text\",\"data\":{\"text\":\"Hi\"}}\n\n")
            append("id: 5\ndata: {\"seq\":5,\"type\":\"run.completed\",\"data\":{\"final_text\":\"Hi\"}}\n\n")
            append("id: 6\ndata: {\"seq\":6,\"type\":\"usage\",\"data\":{}}\n\n")
        }
        server.enqueue(MockResponse().setHeader("Content-Type", "text/event-stream").setBody(body))
        val events = client.turnEvents("t1", after = 2).toList()
        assertEquals(listOf(3, 4, 5), events.map { it.seq }, "the stream ends at the terminal event")
        val request = server.takeRequest()
        assertEquals("/api/turns/t1/events?after=2", request.path)
        assertEquals("text/event-stream", request.getHeader("Accept"))
    }

    @Test
    fun aStreamThatIsRefusedThrows() {
        server.enqueue(json("{}", 401))
        assertFailsWith<ChatException.Unauthorized> { runBlocking { client.notices().toList() } }
    }
}
