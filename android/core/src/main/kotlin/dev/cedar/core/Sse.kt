package dev.cedar.core

import java.io.ByteArrayOutputStream

/** One Server-Sent Events message. */
data class SSEMessage(val event: String = "message", val data: String = "", val id: String? = null)

/**
 * An incremental Server-Sent Events parser. Feed it lines (without their terminator); it returns a
 * message when a blank line ends one. Blank lines carry meaning in SSE, so callers split the byte
 * stream themselves (see [SSELineSplitter]).
 */
class SSEParser {
    private var event: String? = null
    private val data = mutableListOf<String>()
    private var id: String? = null

    fun feed(line: String): SSEMessage? {
        if (line.isEmpty()) {
            val message = if (data.isEmpty()) null else SSEMessage(event ?: "message", data.joinToString("\n"), id)
            event = null
            data.clear()
            id = null
            return message
        }
        if (line.startsWith(":")) return null // comment, e.g. keep-alive

        val colon = line.indexOf(':')
        val field: String
        var value: String
        if (colon >= 0) {
            field = line.substring(0, colon)
            value = line.substring(colon + 1)
            if (value.startsWith(" ")) value = value.drop(1)
        } else {
            field = line
            value = ""
        }
        when (field) {
            "event" -> event = value
            "data" -> data += value
            "id" -> id = value
            else -> {} // includes "retry"
        }
        return null
    }
}

/** Splits a byte stream into lines on `\n`, `\r\n`, or `\r`, keeping empty lines. */
class SSELineSplitter {
    private val buffer = ByteArrayOutputStream()
    private var lastWasCR = false

    /** Returns the line completed by `byte`, if any. */
    fun feed(byte: Byte): String? = when (byte) {
        '\n'.code.toByte() -> {
            if (lastWasCR) {
                lastWasCR = false
                null
            } else flush()
        }
        '\r'.code.toByte() -> {
            lastWasCR = true
            flush()
        }
        else -> {
            lastWasCR = false
            buffer.write(byte.toInt())
            null
        }
    }

    private fun flush(): String {
        val line = String(buffer.toByteArray(), Charsets.UTF_8)
        buffer.reset()
        return line
    }
}
