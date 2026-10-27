package dev.cedar.core

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.job
import kotlinx.coroutines.suspendCancellableCoroutine
import kotlinx.serialization.KSerializer
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import okhttp3.Call
import okhttp3.Callback
import okhttp3.HttpUrl
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import java.io.IOException
import java.net.UnknownServiceException
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLException
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

sealed class ChatException(message: String) : Exception(message) {
    data object NotConfigured : ChatException("Add your cedar-chat server in Settings.")

    data object InvalidURL : ChatException("The server URL is not valid.")

    /**
     * `detail` says what went wrong in English; a `hint`, when there is one, is advice the app words
     * itself in the user's language.
     */
    data class Unreachable(val detail: String, val hint: Hint? = null) : ChatException("Could not reach cedar-chat: $detail") {
        sealed interface Hint {
            /** The address is `http://` (refused as cleartext): use the HTTPS one. */
            data object HttpsOnly : Hint

            /** TLS failed on an explicit port, which likely serves plain HTTP; `error` is the TLS error. */
            data class PlainHttpPort(val port: Int, val error: String) : Hint
        }
    }

    data object Unauthorized : ChatException("cedar-chat wants an access token, or rejected this one. Check Settings.")

    data class Api(val status: Int, val code: String, val text: String) : ChatException(text)
    data class BadResponse(val detail: String) : ChatException("Unexpected response from cedar-chat: $detail")

    val httpStatus: Int? get() = (this as? Api)?.status
}

/**
 * A client for cedar-chat's HTTP API (`/api`), as iOS/Cedar/Networking/ChatClient.swift has it. The
 * browser's page talks to the same routes; the cedar key itself stays on the server.
 */
class ChatClient(val baseURL: HttpUrl, val accessToken: String, private val http: OkHttpClient = defaultHttp) {

    /** Where the web UI shows a conversation, for the "open on the web" links. */
    fun webURL(conversationId: String): String = "${baseURL.toString().trimEnd('/')}/#/c/$conversationId"

    // Server

    suspend fun config(): ServerConfig = decode(request("api/config"), ServerConfig.serializer())

    suspend fun models(): List<ModelInfo> = decode(request("api/models"), ModelList.serializer()).models

    /** Checks that the server is up and takes the token (or needs none). */
    suspend fun verify() {
        val config = config()
        if (config.authRequired && accessToken.isEmpty()) throw ChatException.Unauthorized
        conversations()
    }

    // Conversations

    suspend fun conversations(): List<Conversation> =
        decode(request("api/conversations"), ConversationList.serializer()).conversations

    suspend fun conversation(id: String): ConversationDetail =
        decode(request("api/conversations/$id"), ConversationDetail.serializer())

    suspend fun createConversation(title: String? = null): Conversation {
        val body = buildJsonObject { if (title != null) put("title", title) }
        return decode(request("api/conversations", "POST", body), Conversation.serializer())
    }

    suspend fun rename(conversationId: String, title: String): Conversation =
        decode(request("api/conversations/$conversationId", "PATCH", buildJsonObject { put("title", title) }), Conversation.serializer())

    /** Deletes the conversation and its cedar runs. 409 while a response is running. */
    suspend fun delete(conversationId: String) {
        send(request("api/conversations/$conversationId", "DELETE"))
    }

    /** The conversation has been looked at: its finished responses leave the inbox. */
    suspend fun markSeen(conversationId: String): Conversation =
        decode(request("api/conversations/$conversationId/seen", "POST", JsonObject(emptyMap())), Conversation.serializer())

    /** Starred conversations, most recently starred first: chats and scheduled runs alike. */
    suspend fun starred(): List<Conversation> = decode(request("api/starred"), ConversationList.serializer()).conversations

    /** Stars or unstars the conversation, for every device. Both are idempotent. */
    suspend fun setStarred(conversationId: String, starred: Boolean): Conversation =
        decode(request("api/conversations/$conversationId/star", if (starred) "PUT" else "DELETE"), Conversation.serializer())

    // Schedules

    suspend fun schedules(): List<Schedule> = decode(request("api/schedules"), ScheduleList.serializer()).schedules

    suspend fun schedule(id: String): ScheduleDetail = decode(request("api/schedules/$id"), ScheduleDetail.serializer())

    /** Starts a run now, without moving the next scheduled one. 409 while the latest is going. */
    suspend fun runSchedule(id: String): ScheduleRun =
        decode(request("api/schedules/$id/run", "POST", JsonObject(emptyMap())), ScheduleRun.serializer())

    /** Pauses or resumes. Resuming skips the slots missed while paused. */
    suspend fun setSchedule(id: String, enabled: Boolean): Schedule =
        decode(request("api/schedules/$id", "PATCH", buildJsonObject { put("enabled", enabled) }), Schedule.serializer())

    // Messages and turns

    @Serializable
    data class SendMessage(
        val content: String,
        val model: String? = null,
        val reasoning: String? = null,
        /** PDFs for any model; images for one registered as accepting them. */
        val attachments: List<Upload>? = null,
        /** `off`, `on`, `auto` or `tool`; absent leaves it to the server's Settings. */
        val plan: String? = null,
        /** The user's locale as a BCP 47 tag (`de-CH`), so the agent answers in it ([AppLocales.effective]). */
        val locale: String? = null,
    ) {
        @Serializable
        data class Upload(
            val name: String,
            @SerialName("media_type") val mediaType: String,
            /** The file's bytes, base64. */
            val data: String,
        )
    }

    /** Starts a response. 409 if one is still running in the conversation. */
    suspend fun send(conversationId: String, message: SendMessage): Turn {
        val body = CedarJson.encodeToJsonElement(SendMessage.serializer(), message)
        return decode(request("api/conversations/$conversationId/messages", "POST", body), Turn.serializer())
    }

    suspend fun cancel(turnId: String) {
        send(request("api/turns/$turnId/cancel", "POST", JsonObject(emptyMap())))
    }

    /** Agrees to a run's plan, as proposed or edited. */
    suspend fun answerPlan(turnId: String, steps: List<PlanStep>) {
        val body = buildJsonObject {
            putJsonArray("steps") { steps.forEach { add(buildJsonObject { put("id", it.id); put("title", it.title) }) } }
        }
        send(request("api/turns/$turnId/plan", "POST", body))
    }

    /** `allow`, `allow_always` or `deny`. */
    suspend fun decideApproval(turnId: String, approvalId: String, decision: String) {
        send(request("api/turns/$turnId/approvals/$approvalId", "POST", buildJsonObject { put("decision", decision) }))
    }

    /**
     * Streams a turn's events after `after`: a replay of the stored log, then live events. The flow
     * completes after the terminal event, or when the server closes the connection.
     */
    fun turnEvents(turnId: String, after: Int = 0): Flow<RunEvent> =
        stream(request("api/turns/$turnId/events", query = mapOf("after" to after.toString())), RunEvent.serializer()) { it.isTerminal }

    // Settings

    /**
     * Sets the server's language (`PUT /api/settings/locale`), which scheduled runs and notifications
     * follow; `null` clears it, for an app that follows the device.
     */
    suspend fun setLocale(locale: String?) {
        send(request("api/settings/locale", "PUT", buildJsonObject { put("locale", locale) }))
    }

    // Inbox and notices

    suspend fun inbox(): Inbox = decode(request("api/inbox"), Inbox.serializer())

    /**
     * The server's live notices: a `turn.finished` for every response that ends, with no replay. Read
     * the inbox when connecting. Stays open until cancelled or the connection drops.
     */
    fun notices(): Flow<Notice> = stream(request("api/events"), Notice.serializer()) { false }

    // Files

    suspend fun files(conversationId: String): WorkspaceFiles =
        decode(request("api/conversations/$conversationId/files"), WorkspaceFiles.serializer())

    /** An authorized request for a workspace file, for image loading and sharing. */
    fun fileRequest(conversationId: String, path: String): Request = request("api/conversations/$conversationId/files/$path")

    /** The bytes of a workspace file. */
    suspend fun file(conversationId: String, path: String): ByteArray = send(fileRequest(conversationId, path))

    // Push

    /**
     * Tells the server where to reach this phone: `fcm` with the FCM registration token, or
     * `unifiedpush` with the distributor's https endpoint.
     */
    suspend fun registerAndroidDevice(transport: String, token: String, name: String?): AndroidDevice? {
        val body = buildJsonObject {
            put("transport", transport)
            put("token", token)
            if (!name.isNullOrEmpty()) put("name", name)
        }
        return decode(request("api/notifications/android/devices", "POST", body), AndroidDeviceResponse.serializer()).device
    }

    suspend fun unregisterAndroidDevice(token: String) {
        send(request("api/notifications/android/devices", "DELETE", buildJsonObject { put("token", token) }))
    }

    /** Sends a test message through the notification method the server is set to (`POST /api/notifications/test`). */
    suspend fun testNotification(method: String) {
        send(request("api/notifications/test", "POST", buildJsonObject { put("method", method) }))
    }

    // Plumbing

    internal fun request(path: String, method: String = "GET", json: kotlinx.serialization.json.JsonElement? = null, query: Map<String, String> = emptyMap()): Request {
        val url = baseURL.newBuilder().apply {
            // Each segment is a path component, so it is percent-encoded.
            path.split("/").filter { it.isNotEmpty() }.forEach { addPathSegment(it) }
            query.forEach { (k, v) -> addQueryParameter(k, v) }
        }.build()
        val body = json?.toString()?.toRequestBody(JSON)
        return Request.Builder().url(url).apply {
            if (accessToken.isNotEmpty()) header("Authorization", "Bearer $accessToken")
            method(method, body ?: if (method == "GET") null else ByteArray(0).toRequestBody(null))
        }.build()
    }

    private suspend fun send(request: Request): ByteArray {
        val response = transport { http.newCall(request).await() }
        response.use {
            val body = it.body?.bytes() ?: ByteArray(0)
            if (!it.isSuccessful) throw error(it.code, body)
            return body
        }
    }

    private suspend fun <T> decode(request: Request, serializer: KSerializer<T>): T {
        val data = send(request)
        return try {
            CedarJson.decodeFromString(serializer, data.toString(Charsets.UTF_8))
        } catch (e: Exception) {
            throw ChatException.BadResponse(e.message ?: e.toString())
        }
    }

    /**
     * A Server-Sent Events stream of JSON messages decoded with `serializer`, completing after a
     * message `isLast` says ends it, or when the connection closes.
     */
    private fun <T> stream(request: Request, serializer: KSerializer<T>, isLast: (T) -> Boolean): Flow<T> = flow {
        val call = streamHttp.newCall(request.newBuilder().header("Accept", "text/event-stream").build())
        currentCoroutineContext().job.invokeOnCompletion { call.cancel() }
        val response = transport { call.execute() }
        response.use {
            if (!it.isSuccessful) throw error(it.code, it.body?.bytes() ?: ByteArray(0))
            val source = it.body?.source() ?: return@flow
            val splitter = SSELineSplitter()
            val parser = SSEParser()
            while (true) {
                val byte = try {
                    if (source.exhausted()) break
                    source.readByte()
                } catch (e: IOException) {
                    if (call.isCanceled()) break
                    throw unreachable(e, baseURL)
                }
                val line = splitter.feed(byte) ?: continue
                val message = parser.feed(line) ?: continue
                val item = try {
                    CedarJson.decodeFromString(serializer, message.data)
                } catch (_: Exception) {
                    continue
                }
                emit(item)
                if (isLast(item)) break
            }
        }
    }.flowOn(Dispatchers.IO)

    // The stream stays open for as long as the run goes; the server sends a keep-alive every 15 seconds.
    private val streamHttp: OkHttpClient by lazy { http.newBuilder().readTimeout(60, TimeUnit.SECONDS).callTimeout(0, TimeUnit.SECONDS).build() }

    private inline fun <T> transport(body: () -> T): T = try {
        body()
    } catch (e: IOException) {
        throw unreachable(e, baseURL)
    }

    companion object {
        const val REQUEST_TIMEOUT_SECONDS = 30L
        private val JSON = "application/json".toMediaType()

        val defaultHttp: OkHttpClient by lazy {
            OkHttpClient.Builder()
                .connectTimeout(REQUEST_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                .readTimeout(REQUEST_TIMEOUT_SECONDS, TimeUnit.SECONDS)
                .writeTimeout(120, TimeUnit.SECONDS)
                .build()
        }

        /** Builds a client from the user's settings. */
        fun from(settings: ServerSettings, http: OkHttpClient = defaultHttp): ChatClient {
            val text = settings.serverURL.trim()
            if (text.isEmpty()) throw ChatException.NotConfigured
            val url = text.toHttpUrlOrNull() ?: throw ChatException.InvalidURL
            return ChatClient(url, settings.accessToken.trim(), http)
        }

        /**
         * A TLS failure on an https URL with an explicit port usually means the port serves plain HTTP
         * (cedar-chat itself on :3000), not the `tailscale serve` address, so say so. Cleartext the
         * network security config refused means an `http://` address.
         */
        fun unreachable(error: IOException, baseURL: HttpUrl): ChatException {
            if (error is UnknownServiceException && error.message.orEmpty().contains("CLEARTEXT", ignoreCase = true)) {
                return ChatException.Unreachable(ServerSettings.HTTPS_ONLY, ChatException.Unreachable.Hint.HttpsOnly)
            }
            val detail = error.message ?: error.javaClass.simpleName
            val explicitPort = baseURL.port != HttpUrl.defaultPort(baseURL.scheme)
            if (error !is SSLException || !baseURL.isHttps || !explicitPort || baseURL.port == 443) {
                return ChatException.Unreachable(detail)
            }
            return ChatException.Unreachable(
                "$detail Port ${baseURL.port} may serve plain HTTP. Use the HTTPS address " +
                    "from tailscale serve, e.g. https://<machine>.<tailnet>.ts.net (no port).",
                ChatException.Unreachable.Hint.PlainHttpPort(baseURL.port, detail),
            )
        }

        fun error(status: Int, body: ByteArray): ChatException {
            if (status == 401) return ChatException.Unauthorized
            val envelope = parseJson(body.toString(Charsets.UTF_8))
            val code = envelope["error"]["code"].str
            val message = envelope["error"]["message"].str
            if (code != null && message != null) return ChatException.Api(status, code, message)
            return ChatException.Api(status, "http_$status", "cedar-chat returned HTTP $status")
        }
    }
}

/** Runs the call on OkHttp's dispatcher, cancelling it with the coroutine. */
suspend fun Call.await(): Response = suspendCancellableCoroutine { cont ->
    cont.invokeOnCancellation { cancel() }
    enqueue(object : Callback {
        override fun onFailure(call: Call, e: IOException) {
            cont.resumeWithException(e)
        }

        override fun onResponse(call: Call, response: Response) {
            cont.resume(response)
        }
    })
}
