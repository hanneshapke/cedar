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
    fun starsSchedulesAndRegistersThePhone() = runBlocking {
        val conversation = """{"id":"c1","unread":false,"needs_answer":false,"starred_at":"2026-10-23T08:00:00Z",
            "created_at":"2026-10-12T03:36:00Z","updated_at":"2026-10-12T03:36:00Z"}"""
        server.enqueue(json(conversation))
        server.enqueue(json(conversation))
        server.enqueue(json("""{"id":"s1","title":"News","frequency":"daily","time_of_day":"07:30","timezone":"UTC","enabled":false,"next_run_at":"2026-10-26T06:30:00Z"}"""))
        server.enqueue(json("""{"device":{"id":"d1","transport":"unifiedpush","name":"Pixel"},"device_id":"d1"}"""))
        server.enqueue(json("{}"))

        client.setStarred("c1", true)
        client.setStarred("c1", false)
        client.setSchedule("s1", enabled = false)
        val device = client.registerAndroidDevice("unifiedpush", "https://ntfy.sh/upAbc", "Pixel")
        client.unregisterAndroidDevice("https://ntfy.sh/upAbc")

        assertEquals("PUT /api/conversations/c1/star", server.takeRequest().let { "${it.method} ${it.path}" })
        assertEquals("DELETE /api/conversations/c1/star", server.takeRequest().let { "${it.method} ${it.path}" })
        server.takeRequest().let {
            assertEquals("PATCH /api/schedules/s1", "${it.method} ${it.path}")
            assertEquals(j("""{"enabled":false}"""), j(it.body.readUtf8()))
        }
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
