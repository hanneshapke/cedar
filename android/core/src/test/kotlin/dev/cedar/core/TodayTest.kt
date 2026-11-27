package dev.cedar.core

import kotlinx.coroutines.runBlocking
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import java.time.DayOfWeek
import java.time.LocalDateTime
import kotlin.test.AfterTest
import kotlin.test.BeforeTest
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** Today: the river, an ask's outcome and asking with a When, as chat/src/api/{timeline,outcome,asks}.rs send and take them. */
class TodayTest {
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

    private val timeline = """{
      "day":"2026-11-27","timezone":"Europe/Zurich","today":true,"generated_at":"2026-11-27T09:42:00Z","needs_you":3,
      "earlier":[
        {"turn_id":"t1","conversation_id":"c1","title":"Morning brief","prompt":"Brief me","state":"done","status":"completed",
         "stop_reason":"end_turn","summary":"Rain until noon.","error":null,"created_at":"2026-11-27T06:00:00Z",
         "finished_at":"2026-11-27T06:01:00Z","seen":true,"needs_answer":false,
         "agent":{"id":"a1","title":"Morning brief","trigger_kind":"time"},"waits":[]},
        {"turn_id":"t2","conversation_id":"c2","title":null,"prompt":" Check the strike ","state":"failed","status":"failed",
         "stop_reason":null,"summary":null,"error":"The search service didn't answer.","created_at":"2026-11-27T07:00:00Z",
         "finished_at":"2026-11-27T07:00:30Z","seen":false,"needs_answer":false,"agent":null,"waits":[]}],
      "now":[
        {"turn_id":"t3","conversation_id":"c3","title":"Mail the landlord","prompt":"Mail the landlord","state":"waiting","status":"running",
         "summary":null,"created_at":"2026-11-27T09:30:00Z","finished_at":null,"seen":false,"needs_answer":false,"agent":null,
         "waits":[{"kind":"approval","key":"approval:a1","reason":"Send a message in your Gmail.","since":"2026-11-27T09:31:00Z"},
                  {"kind":"plan","key":"plan","reason":null,"since":"2026-11-27T09:30:10Z"}]},
        {"turn_id":"t4","conversation_id":"c4","title":"Cancel the gym","prompt":"Cancel the gym","state":"asking","status":"completed",
         "stop_reason":"ask_user","summary":"By letter or by email?","created_at":"2026-11-27T09:35:00Z","needs_answer":true,"waits":[]},
        {"turn_id":"t5","conversation_id":"c5","title":"Later","prompt":"x","state":"pondering","status":"running","waits":[]}],
      "outbox":[{"id":"h1","run_id":"r9","call_id":"t1","tool":"gmail_send_message","input":{"to":"ana@example.com","subject":"Digest"},
         "reason":"Send message in your Gmail to ana@example.com.","why":"unattended","status":"pending","edited":false,"output":null,
         "created_at":"2026-11-27T08:00:00Z","decided_at":null,"turn_id":"t1","conversation_id":"c1","title":"Morning brief",
         "agent":{"id":"a1","title":"Morning brief"}}],
      "ahead":[{"id":"a1","title":"Morning brief","enabled":true,"trigger_kind":"time",
         "trigger":{"kind":"time","frequency":"weekdays","time":"07:00","timezone":"Europe/Zurich"},"watches":false,
         "next_run_at":"2026-11-30T06:00:00Z","last_run_at":"2026-11-27T06:00:00Z","last_checked_at":null}],
      "watching":[{"id":"a2","title":"File receipts","enabled":true,"trigger_kind":"email",
         "trigger":{"kind":"email","query":"receipt","every_minutes":15},"watches":true,"next_run_at":null,
         "last_run_at":null,"last_checked_at":"2026-11-27T09:40:00Z"}],
      "standing":[{"id":"a3","title":"From a newer server","enabled":false,"trigger_kind":"moon","trigger":null,"watches":false,
         "next_run_at":null,"last_run_at":null,"last_checked_at":null}]}"""

    @Test
    fun readsADayOfTheRiver() = runBlocking {
        server.enqueue(json(timeline))
        val day = client.timeline(day = "2026-11-27", timezone = "Europe/Zurich")
        val request = server.takeRequest()
        assertEquals("/api/timeline?day=2026-11-27&timezone=Europe%2FZurich", request.path)
        assertEquals("Bearer tok", request.getHeader("Authorization"))

        assertEquals("2026-11-27", day.day)
        assertTrue(day.today)
        assertEquals(3, day.needsYou)
        assertFalse(day.isEmpty)

        // Earlier: an agent's finished run, and a chat that failed and has no title.
        val (brief, strike) = day.earlier
        assertEquals(TurnState.DONE, brief.state)
        assertEquals("Morning brief", brief.label)
        assertEquals("Rain until noon.", brief.line)
        assertEquals("a1", brief.agent?.id)
        assertEquals(TurnState.FAILED, strike.state)
        assertEquals("Check the strike", strike.label, "what was asked stands in for a missing title")
        assertEquals("The search service didn't answer.", strike.line)
        assertFalse(strike.state.needsYou)

        // Now: an approval and a plan to agree to, a question, and a state this app doesn't know.
        val (waiting, asking, newer) = day.now
        assertEquals(TurnState.WAITING, waiting.state)
        assertTrue(waiting.state.needsYou)
        assertEquals(listOf("a1", null), waiting.waits.map { it.approvalId })
        assertTrue(waiting.waits[0].isApproval && waiting.waits[1].isPlan)
        assertEquals("Send a message in your Gmail.", waiting.waits[0].reason)
        assertEquals(TurnState.ASKING, asking.state)
        assertTrue(asking.needsAnswer && asking.state.needsYou)
        assertEquals("By letter or by email?", asking.line)
        assertEquals(TurnState.OTHER, newer.state)

        // What an agent's run held, with the turn its outcome is under.
        val held = day.outbox.single()
        assertTrue(held.isOpen)
        assertEquals("t1", held.turnId)
        assertEquals("Morning brief", held.origin)
        assertEquals(listOf("to" to "ana@example.com", "subject" to "Digest"), held.fields)

        // The agents are the app's agents: a clock has a next run, a watcher a last check.
        val clock = day.ahead.single()
        assertEquals(Trigger.Time("weekdays", null, "07:00", "Europe/Zurich"), clock.trigger)
        assertTrue(clock.status is Agent.Status.NextRun)
        assertNotNull(clock.nextRunAt)
        val watcher = day.watching.single()
        assertEquals(Trigger.Email("receipt", 15), watcher.trigger)
        assertTrue(watcher.trigger.byEvent)
        assertTrue(watcher.status is Agent.Status.Watching)
        // A trigger a newer server sends as nothing readable is still an agent in the list.
        val unknown = day.standing.single()
        assertTrue(unknown.trigger is Trigger.Unknown)
        assertFalse(unknown.enabled)
    }

    @Test
    fun anotherDayAndAnOlderServer() = runBlocking {
        // A past day has only what happened; today's request names no day.
        server.enqueue(json("""{"day":"2001-02-03","timezone":"UTC","today":false,"needs_you":0,"earlier":[],"now":[],"ahead":[],"watching":[],"standing":[]}"""))
        val past = client.timeline(timezone = "UTC")
        assertEquals("/api/timeline?timezone=UTC", server.takeRequest().path)
        assertTrue(past.isEmpty && !past.today)
        assertEquals(emptyList(), past.outbox, "a server from before the outbox in the river sends none")

        // A server from before Today has no such route.
        server.enqueue(json("""{"error":{"code":"not_found","message":"no such route"}}""", 404))
        val missing = runCatching { client.timeline() }.exceptionOrNull() as? ChatException.Api
        assertEquals(404, missing?.status)
    }

    @Test
    fun readsWhatAnAskCameTo() = runBlocking {
        server.enqueue(json("""{
          "turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"Plan Lisbon","status":"completed",
                  "final_text":"## Lisbon\n\nFriday 13 to Sunday 15 March works.","error":null,"stop_reason":"end_turn",
                  "finished_at":"2026-11-27T10:58:00Z","seen":true,"attachments":[],"rating":null,"created_at":"2026-11-27T10:31:00Z"},
          "conversation":{"id":"c1","title":"Plan Lisbon","private":false},
          "agent":null,"state":"done","summary":"Friday 13 to Sunday 15 March works.","later_turns":1,
          "steps":[{"id":"s1","name":"web_search","is_error":false},{"id":"s2","name":"bash","is_error":true}],
          "sources":[{"url":"https://visit.example/lisbon","path":null,"title":"Visit Lisbon"},
                     {"url":null,"path":"out/notes.md","title":null},{"url":"javascript:alert(1)","path":null,"title":null}],
          "followups":[{"call_id":"s9","output":"{\"follow_up\":{\"title\":\"Fares\",\"prompt\":\"Check fares\",\"at\":\"2099-01-01T09:00:00Z\"}}"},
                       {"call_id":"s10","output":"not a suggestion"}],
          "followup_decisions":[{"turn_id":"t1","call_id":"s9","status":"dismissed","agent_id":null,"decided_at":"2026-11-27T11:00:00Z"}],
          "memories":[{"id":"m1","text":"Prefers walking to taxis","kind":"preference"}],
          "records":[{"set":"trips","key":"lisbon","data":{"city":"Lisbon"}},{"set":"log","key":null,"data":{}}],
          "files":[{"path":"out/plan.md","size":120,"modified_at":"2026-11-27T10:57:00Z"}],
          "held":[{"id":"h2","run_id":"r1","tool":"gmail_send_message","input":{},"reason":"Sent","why":"unattended","status":"sent",
                   "created_at":"2026-11-27T10:40:00Z","decided_at":"2026-11-27T10:50:00Z"},
                  {"id":"h1","run_id":"r1","tool":"gmail_send_message","input":{},"reason":"Send","why":"unattended","status":"pending",
                   "created_at":"2026-11-27T10:45:00Z","decided_at":null}]}"""))
        val outcome = client.outcome("t1")
        assertEquals("/api/turns/t1/outcome", server.takeRequest().path)

        assertEquals(TurnState.DONE, outcome.state)
        assertEquals("Friday 13 to Sunday 15 March works.", outcome.headline)
        assertTrue(outcome.turn.finalText.orEmpty().contains("Lisbon"))
        assertEquals(1, outcome.laterTurns)
        assertEquals(listOf("web_search", "bash"), outcome.steps.map { it.name })
        assertEquals(1, outcome.failedSteps)
        // Sources: a page opens, a document has no address, and only http(s) is a link.
        assertEquals(listOf("Visit Lisbon", "out/notes.md", "javascript:alert(1)"), outcome.sources.map { it.label })
        assertEquals(listOf("https://visit.example/lisbon", null, null), outcome.sources.map { it.link })
        // A follow-up reads as the conversation's does; what isn't one is nothing.
        assertEquals("Fares", outcome.followups[0].suggestion?.title)
        assertNull(outcome.followups[1].suggestion)
        assertTrue(outcome.decision("s9")?.isDismissed == true)
        assertNull(outcome.decision("s10"))
        assertEquals("Prefers walking to taxis", outcome.memories.single().text)
        assertEquals(listOf("trips: lisbon", "log"), outcome.records.map { it.label })
        assertEquals("out/plan.md", outcome.files.single().path)
        assertEquals(listOf("h1", "h2"), outcome.heldOpenFirst.map { it.id }, "what still waits comes first")
    }

    @Test
    fun anOutcomeThatFailedOrIsStillGoing() {
        val failed = CedarJson.decodeFromString(Outcome.serializer(), """{
          "turn":{"id":"t1","conversation_id":"c1","run_id":"r1","prompt":"Check the strike","status":"failed","error":"No answer."},
          "conversation":{"id":"c1","title":"Check the strike","private":true},"state":"failed","summary":null}""")
        assertEquals("No answer.", failed.headline)
        assertTrue(failed.conversation.isPrivate)
        // Still going: nothing of the run is read yet, and the headline is what was asked.
        val going = CedarJson.decodeFromString(Outcome.serializer(), """{
          "turn":{"id":"t2","conversation_id":"c2","run_id":"r2","prompt":" Summarise the lease ","status":"running"},
          "conversation":{"id":"c2","title":null},"state":"working"}""")
        assertEquals(TurnState.WORKING, going.state)
        assertEquals("Summarise the lease", going.headline)
        assertTrue(going.steps.isEmpty() && going.held.isEmpty() && going.files.isEmpty())
    }

    @Test
    fun anAskNowIsAConversationAndCanBeOffTheRecord() = runBlocking {
        server.enqueue(json("""{"when":"now",
          "conversation":{"id":"c1","title":"Hello","private":true,"created_at":"2026-11-27T10:00:00Z","updated_at":"2026-11-27T10:00:00Z"},
          "turn":{"id":"t1","conversation_id":"c1","seq":1,"run_id":"r1","prompt":"Hello","status":"queued"}}""", 201))
        val draft = AskDraft(private = true)
        assertNull(draft.problem("Hello"))
        val result = client.ask(draft.body("  Hello ", "Europe/Zurich", "de-CH"))
        val request = server.takeRequest()
        assertEquals("POST", request.method)
        assertEquals("/api/asks", request.path)
        assertEquals(j("""{"content":"Hello","private":true,"locale":"de-CH","timezone":"Europe/Zurich"}"""), j(request.body.readUtf8()))
        assertEquals("c1", result.conversation?.id)
        assertEquals(TurnStatus.QUEUED, result.turn?.status)
        assertNull(result.agent)
        // In the open, nothing says private.
        assertEquals(j("""{"content":"Hi","timezone":"UTC"}"""), j(AskDraft().body("Hi", "UTC").toJson().toString()))
    }

    @Test
    fun anAskForLaterBecomesAnAgent() = runBlocking {
        server.enqueue(json("""{"when":"later","agent":{"id":"a1","title":"Call the dentist","prompt":"Call the dentist","enabled":true,
          "trigger":{"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Zurich"},"next_run_at":"2026-12-03T08:00:00Z"}}""", 201))
        val draft = AskDraft(whenKind = AskWhen.LATER, laterAt = LocalDateTime.of(2026, 12, 3, 9, 0, 42), private = true)
        val result = client.ask(draft.body("Call the dentist", "Europe/Zurich", "en"))
        assertEquals(
            j("""{"content":"Call the dentist","when":"later","trigger":{"kind":"once","at":"2026-12-03T09:00","timezone":"Europe/Zurich"},"timezone":"Europe/Zurich"}"""),
            j(server.takeRequest().body.readUtf8()),
            "no seconds in the time, and nothing off the record or of the locale with a When that isn't now",
        )
        assertEquals("later", result.whenId)
        assertEquals(Trigger.Once("2026-12-03T09:00", "Europe/Zurich"), result.agent?.trigger)
        assertNull(result.conversation)
        assertFalse(draft.isPrivate)
    }

    @Test
    fun aWhenBuildsTheTriggerThatFitsIt() {
        val zone = "Europe/Zurich"
        assertNull(AskDraft().trigger(zone))
        // Every: daily and weekdays carry no weekday; weekly does.
        assertEquals(Trigger.Time("daily", null, "07:00", zone), AskDraft(whenKind = AskWhen.EVERY).trigger(zone))
        assertEquals(Trigger.Time("weekdays", null, "18:30", zone), AskDraft(whenKind = AskWhen.EVERY, frequency = AskFrequency.WEEKDAYS, weekday = 4, time = "18:30").trigger(zone))
        assertEquals(
            Trigger.Time("weekly", 6, "07:00", zone),
            AskDraft(whenKind = AskWhen.EVERY, frequency = AskFrequency.WEEKLY, weekday = AskDraft.weekday(DayOfWeek.SUNDAY)).trigger(zone),
        )
        assertEquals(0, AskDraft.weekday(DayOfWeek.MONDAY))
        // Watch: mail matching a search, before calendar events, or the person's own device.
        val watch = AskDraft(whenKind = AskWhen.WATCH)
        assertNull(watch.trigger(zone), "no search yet")
        assertEquals(Trigger.Email("from:bank"), watch.copy(query = " from:bank ").trigger(zone))
        assertEquals(Trigger.Calendar(minutesBefore = 30), watch.copy(watch = AskWatch.CALENDAR).trigger(zone))
        assertNull(watch.copy(watch = AskWatch.CALENDAR, minutesBefore = 0).trigger(zone))
        assertEquals(Trigger.Device, watch.copy(watch = AskWatch.DEVICE).trigger(zone))
        // What goes out is what the server's triggers read.
        assertEquals(
            j("""{"content":"File receipts","when":"watch","trigger":{"kind":"email","query":"from:bank","every_minutes":${AgentLimits.DEFAULT_CHECK_MINUTES}},"timezone":"UTC"}"""),
            j(watch.copy(query = "from:bank").body("File receipts", "UTC").toJson().toString()),
        )
        assertEquals(
            j("""{"kind":"calendar","minutes_before":45,"new_events":false,"every_minutes":${AgentLimits.DEFAULT_CHECK_MINUTES}}"""),
            j(watch.copy(watch = AskWatch.CALENDAR, minutesBefore = 45).trigger(zone)!!.toJson().toString()),
        )
    }

    @Test
    fun aDraftSaysWhatIsMissing() {
        val now = LocalDateTime.of(2026, 11, 27, 10, 42)
        assertEquals(AskProblem.NO_TEXT, AskDraft().problem("  ", now))
        assertNull(AskDraft().problem("Hi", now))
        assertEquals(AskProblem.NO_TIME, AskDraft(whenKind = AskWhen.LATER).problem("Hi", now))
        assertEquals(AskProblem.TIME_PASSED, AskDraft(whenKind = AskWhen.LATER, laterAt = now.minusMinutes(1)).problem("Hi", now))
        assertEquals(AskProblem.TIME_PASSED, AskDraft(whenKind = AskWhen.LATER, laterAt = now).problem("Hi", now))
        assertEquals(AskProblem.NO_QUERY, AskDraft(whenKind = AskWhen.WATCH).problem("Hi", now))
        assertEquals(AskProblem.BAD_MINUTES, AskDraft(whenKind = AskWhen.WATCH, watch = AskWatch.CALENDAR, minutesBefore = 2000).problem("Hi", now))
        assertNull(AskDraft(whenKind = AskWhen.WATCH, watch = AskWatch.DEVICE).problem("Hi", now))
        assertNull(AskDraft(whenKind = AskWhen.EVERY).problem("Hi", now))
        // The form starts with later at the next full hour, two hours on.
        val start = AskDraft.starting(now)
        assertEquals(LocalDateTime.of(2026, 11, 27, 12, 0), start.laterAt)
        assertNull(start.copy(whenKind = AskWhen.LATER).problem("Hi", now))
        // Off the record is now's only.
        assertTrue(AskDraft(private = true).isPrivate)
        assertFalse(AskDraft(whenKind = AskWhen.EVERY, private = true).isPrivate)
    }

    @Test
    fun statesAndWaitsReadWhatTheyKnow() {
        assertEquals(TurnState.WAITING, TurnState.of("waiting"))
        assertEquals(TurnState.OTHER, TurnState.of(""))
        assertEquals(TurnState.OTHER, TurnState.of("pondering"))
        assertEquals(listOf(TurnState.WAITING, TurnState.ASKING), TurnState.entries.filter { it.needsYou })
        assertNull(TurnWait(kind = "approval", key = "approval:").approvalId)
        assertNull(TurnWait(kind = "approval", key = "a1").approvalId, "a key that isn't an approval's names none")
        assertEquals("a1", TurnWait(kind = "approval", key = "approval:a1").approvalId)
    }
}
