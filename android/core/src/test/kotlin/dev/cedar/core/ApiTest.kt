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
    fun decodesSchedulesAndRuns() {
        val list = CedarJson.decodeFromString(ScheduleList.serializer(), """
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
        """)
        assertEquals(2, list.schedules.size)
        val news = list.schedules[0]
        assertEquals("Mondays at 07:30 (Mars/Olympus)", news.cadence())
        assertEquals(ScheduleRun.Outcome.COMPLETED, news.lastRun?.outcome)
        assertEquals(true, news.lastRun?.unread)
        assertFalse(list.schedules[1].enabled)
        assertNull(list.schedules[1].lastRun)
        assertEquals("Daily at 09:00", list.schedules[1].cadence(deviceZone = "UTC"))

        val detail = CedarJson.decodeFromString(ScheduleDetail.serializer(), """
        {"schedule":{"id":"s1","title":"News","prompt":"p","frequency":"weekdays","weekday":null,"time_of_day":"07:30",
                     "timezone":"Mars/Olympus","enabled":true,"next_run_at":"2026-10-26T06:30:00Z"},
         "runs":[{"id":"r2","schedule_id":"s1","conversation_id":null,"turn_id":null,"due_at":"2026-10-20T06:30:00Z",
                  "started_at":"2026-10-20T07:30:00Z","manual":true,"error":"cedar is busy","turn":null}]}
        """)
        assertEquals("Weekdays at 07:30 (Mars/Olympus)", detail.schedule.cadence())
        assertEquals(ScheduleRun.Outcome.NOT_STARTED, detail.runs.first().outcome)
        assertEquals(false, detail.runs.first().unread)
        assertNull(detail.runs.first().conversationId)
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
        assertEquals(ChatException.Unreachable(ServerSettings.HTTPS_ONLY), ChatClient.unreachable(cleartext, "http://chat.local".toHttpUrl()))
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
        assertTrue(ServerSettings("https://a", "t").sameServer(ServerSettings(" https://a ", "t ", model = "m")))
        assertFalse(ServerSettings("https://a", "t").sameServer(ServerSettings("https://a", "u")))
    }
}
