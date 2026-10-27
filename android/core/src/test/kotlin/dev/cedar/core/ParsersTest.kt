package dev.cedar.core

import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNotNull
import kotlin.test.assertNull
import kotlin.test.assertTrue

// Ported from iOS/CedarTests/SSEParserTests.swift, MarkdownBlockTests.swift, ConnectionCodeTests.swift
// and AttachmentsTests.swift.

class SseTest {
    private fun messages(raw: String): List<SSEMessage> {
        val splitter = SSELineSplitter()
        val parser = SSEParser()
        val out = mutableListOf<SSEMessage>()
        for (byte in raw.toByteArray()) {
            val line = splitter.feed(byte) ?: continue
            parser.feed(line)?.let(out::add)
        }
        return out
    }

    @Test
    fun parsesEventsSeparatedByBlankLines() {
        val raw = "id: 1\nevent: run.started\ndata: {\"seq\":1}\n\n: keep-alive\n\nid: 2\nevent: usage\ndata: {\"seq\":2}\n\n"
        assertEquals(
            listOf(SSEMessage("run.started", "{\"seq\":1}", "1"), SSEMessage("usage", "{\"seq\":2}", "2")),
            messages(raw),
        )
    }

    @Test
    fun handlesCRLFAndMultilineData() {
        assertEquals(
            listOf(SSEMessage("message", "a\nb", null), SSEMessage("message", "c", null)),
            messages("data: a\r\ndata: b\r\n\r\ndata:c\r\r"),
        )
    }

    @Test
    fun incompleteMessageIsNotDispatched() {
        assertEquals(emptyList(), messages("data: partial\n"))
    }

    @Test
    fun keepsMultibyteCharacters() {
        assertEquals(listOf(SSEMessage("message", "Café → ok", null)), messages("data: Café → ok\n\n"))
    }

    @Test
    fun decodesRunEvent() {
        val json = """{"seq":3,"type":"tool.call","data":{"id":"t1","name":"bash","input":{"command":"ls"}},"created_at":"2026-10-12T03:36:00.123456789Z"}"""
        val event = CedarJson.decodeFromString(RunEvent.serializer(), json)
        assertEquals(3, event.seq)
        assertEquals("tool.call", event.type)
        assertEquals("ls", event.data["input"]["command"].str)
    }
}

class MarkdownTest {
    @Test
    fun parsesCommonBlocks() {
        val source = """
            # Title

            Some **bold** text
            on two lines.

            ```python
            print("hi")

            x = 1
            ```

            - one
            - two
              continued
            3. three

            > quoted

            | a | b |
            |---|:-:|
            | 1 | 2 |
            | 3 |

            ---
        """.trimIndent()
        assertEquals(
            listOf(
                MarkdownBlock.Heading(1, "Title"),
                MarkdownBlock.Paragraph("Some **bold** text\non two lines."),
                MarkdownBlock.Code("python", "print(\"hi\")\n\nx = 1"),
                MarkdownBlock.ListBlock(false, 0, listOf("one", "two\ncontinued")),
                MarkdownBlock.ListBlock(true, 3, listOf("three")),
                MarkdownBlock.Quote("quoted"),
                MarkdownBlock.Table(listOf("a", "b"), listOf(listOf("1", "2"), listOf("3", ""))),
                MarkdownBlock.Rule,
            ),
            MarkdownBlock.parse(source),
        )
    }

    @Test
    fun unclosedFenceRunsToEnd() {
        assertEquals(listOf(MarkdownBlock.Code("", "let x = 1")), MarkdownBlock.parse("```\nlet x = 1"))
    }

    @Test
    fun boldIsNotAListOrRule() {
        assertEquals(listOf(MarkdownBlock.Paragraph("**note** this")), MarkdownBlock.parse("**note** this"))
    }
}

class ConnectionCodeTest {
    @Test
    fun readsThePairingLink() {
        // As pairing/src/connect.rs writes it: form-encoded, so ':' and '/' are escaped.
        val code = ConnectionCode.parse("cedar://connect?server=https%3A%2F%2Fmy-mac.tail1234.ts.net&token=ab%26c%3Dd")
        assertEquals(ConnectionCode("https://my-mac.tail1234.ts.net", "ab&c=d"), code)
        assertEquals("https://m.t.ts.net:8443", ConnectionCode.parse("cedar://connect?server=https://m.t.ts.net:8443")?.serverURL)
        assertEquals("", ConnectionCode.parse("cedar://connect?server=https://m.t.ts.net")?.accessToken)
        assertEquals("a+b", ConnectionCode.parse("cedar://connect?server=https://m.t.ts.net&token=a%2Bb")?.accessToken)
    }

    @Test
    fun turnsDownOtherCodes() {
        assertNull(ConnectionCode.parse("https://example.com"))
        assertNull(ConnectionCode.parse("cedar://other?server=https://m.t.ts.net"))
        assertNull(ConnectionCode.parse("cedar://connect?token=abc"))
        // The app connects over HTTPS only.
        assertNull(ConnectionCode.parse("cedar://connect?server=http://127.0.0.1:13000&token=abc"))
        assertNull(ConnectionCode.parse("not a url at all %%"))
    }
}

class AttachmentsTest {
    private fun file(name: String, type: String, size: Int) = OutgoingAttachment(name, type, ByteArray(size))

    @Test
    fun typesTheServerTakes() {
        assertTrue(OutgoingAttachment.accepts("application/pdf"))
        assertTrue(OutgoingAttachment.accepts("image/JPEG"))
        assertTrue(!OutgoingAttachment.accepts("image/heic"))
        assertTrue(!OutgoingAttachment.accepts(null))
        assertEquals("IMG_1.jpg", OutgoingAttachment.jpegName("IMG_1.heic"))
        assertEquals("photo.jpg", OutgoingAttachment.jpegName(".heic"))
        assertEquals("scan.jpg", OutgoingAttachment.jpegName("scan"))
        assertTrue(!file("a.pdf", "application/pdf", 1).isImage)
    }

    @Test
    fun limits() {
        val pdf = file("a.pdf", "application/pdf", 10)
        val image = file("a.png", "image/png", 10)
        assertNull(OutgoingAttachment.problem(pdf, emptyList(), imagesAllowed = false))
        assertEquals("a.png: this model takes PDFs but not images", OutgoingAttachment.problem(image, emptyList(), imagesAllowed = false))
        assertNull(OutgoingAttachment.problem(image, emptyList(), imagesAllowed = true))

        assertNotNull(OutgoingAttachment.problem(file("e.pdf", "application/pdf", 0), emptyList(), imagesAllowed = true))

        val full = List(OutgoingAttachment.MAX_FILES) { file("p$it.pdf", "application/pdf", 10) }
        assertEquals("At most 10 files per message", OutgoingAttachment.problem(pdf, full, imagesAllowed = true))
        assertEquals(OutgoingAttachment.Problem.TooMany(10), OutgoingAttachment.check(pdf, full, imagesAllowed = true))
        assertEquals(OutgoingAttachment.Problem.ImagesNotAccepted("a.png"), OutgoingAttachment.check(image, emptyList(), imagesAllowed = false))

        val big = file("b.pdf", "application/pdf", OutgoingAttachment.MAX_BYTES)
        assertNull(OutgoingAttachment.problem(big, emptyList(), imagesAllowed = true))
        assertNotNull(OutgoingAttachment.problem(big, listOf(big), imagesAllowed = true))
        assertNotNull(OutgoingAttachment.problem(file("c.pdf", "application/pdf", OutgoingAttachment.MAX_BYTES + 1), emptyList(), true))
    }

    @Test
    fun encodesAttachmentsTheWayTheServerReadsThem() {
        val message = ChatClient.SendMessage("look", attachments = listOf(ChatClient.SendMessage.Upload("a.pdf", "application/pdf", "JVBERg==")))
        val json = CedarJson.encodeToJsonElement(ChatClient.SendMessage.serializer(), message)
        assertNull(json["model"])
        assertEquals(j("""[{"name":"a.pdf","media_type":"application/pdf","data":"JVBERg=="}]"""), json["attachments"])
        val plain = CedarJson.encodeToJsonElement(ChatClient.SendMessage.serializer(), ChatClient.SendMessage("hi"))
        assertNull(plain["attachments"])
        // No plan mode means the server's Settings decide, so none is sent.
        assertNull(plain["plan"])
        val planned = CedarJson.encodeToJsonElement(ChatClient.SendMessage.serializer(), ChatClient.SendMessage("hi", plan = "tool"))
        assertEquals(j("\"tool\""), planned["plan"])
        assertEquals(listOf("", "tool", "auto", "on", "off"), PLAN_MODES.map { it.first })
    }
}
