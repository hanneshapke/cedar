package dev.cedar.core

import kotlinx.serialization.KSerializer
import kotlinx.serialization.descriptors.PrimitiveKind
import kotlinx.serialization.descriptors.PrimitiveSerialDescriptor
import kotlinx.serialization.encoding.Decoder
import kotlinx.serialization.encoding.Encoder
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.doubleOrNull
import java.time.Instant
import java.time.OffsetDateTime

/** Decodes the server's payloads: unknown fields are ignored, as the iOS app's `Decodable`s do. */
val CedarJson = Json {
    ignoreUnknownKeys = true
    explicitNulls = false
    coerceInputValues = true
    isLenient = false
}

// Arbitrary JSON (event payloads, tool inputs, card results) is read through these, the way the
// iOS app reads its `JSONValue`: a missing key, a wrong type or a null is just `null`.

operator fun JsonElement?.get(key: String): JsonElement? = (this as? JsonObject)?.get(key)

/** A JSON string; numbers and booleans are not strings. */
val JsonElement?.str: String?
    get() = (this as? JsonPrimitive)?.takeIf { it.isString }?.content

/** A finite JSON number. */
val JsonElement?.dbl: Double?
    get() = (this as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull?.takeIf { it.isFinite() }

/** A number, truncated as Swift's `Int(Double)` does. */
val JsonElement?.int: Int?
    get() = dbl?.let { if (it >= Int.MIN_VALUE && it <= Int.MAX_VALUE) it.toInt() else null }

val JsonElement?.bool: Boolean?
    get() = (this as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull

val JsonElement?.arr: List<JsonElement>?
    get() = this as? JsonArray

val JsonElement?.isNull: Boolean
    get() = this == null || this is JsonNull

/** Parses JSON text, or `null` when it isn't JSON. */
fun parseJson(text: String): JsonElement? = try {
    CedarJson.parseToJsonElement(text)
} catch (_: Exception) {
    null
}

private val prettyJson = Json { prettyPrint = true }

/** Pretty-printed JSON, for showing tool inputs; a string is shown as itself. */
fun JsonElement?.pretty(): String = when {
    this == null -> ""
    this is JsonPrimitive && isString -> content
    else -> sortKeys(this).let { prettyJson.encodeToString(JsonElement.serializer(), it) }
}

private fun sortKeys(e: JsonElement): JsonElement = when (e) {
    is JsonObject -> JsonObject(e.toSortedMap().mapValues { sortKeys(it.value) })
    is JsonArray -> JsonArray(e.map(::sortKeys))
    else -> e
}

object CedarDate {
    /** Parses RFC 3339 timestamps with any fractional precision (chrono writes up to nine digits). */
    fun parse(raw: String): Instant? = try {
        OffsetDateTime.parse(raw).toInstant()
    } catch (_: Exception) {
        null
    }
}

/** RFC 3339 timestamps as `Instant`s. */
object InstantSerializer : KSerializer<Instant> {
    override val descriptor = PrimitiveSerialDescriptor("dev.cedar.Instant", PrimitiveKind.STRING)

    override fun deserialize(decoder: Decoder): Instant {
        val raw = decoder.decodeString()
        return CedarDate.parse(raw) ?: throw IllegalArgumentException("bad date $raw")
    }

    override fun serialize(encoder: Encoder, value: Instant) = encoder.encodeString(value.toString())
}
