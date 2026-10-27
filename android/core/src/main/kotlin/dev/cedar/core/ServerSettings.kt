package dev.cedar.core

import java.net.URI

/**
 * Where cedar-chat is and how to talk to it. The app keeps the access token encrypted with an
 * Android Keystore key, the rest in its preferences.
 */
data class ServerSettings(
    /** cedar-chat's address, as the browser reaches it. */
    val serverURL: String = "",
    /** Its `CHAT_ACCESS_TOKEN`; empty when the server runs without one. */
    val accessToken: String = "",
    /** The registry id of the model to use for new messages, or empty for the server's default. */
    val model: String = "",
    /** The reasoning level to ask for, or empty for the model's default. */
    val reasoning: String = "",
    /** Whether messages' runs plan (one of [PLAN_MODES]), or empty for the server's Settings. */
    val plan: String = "",
) {
    val isConfigured: Boolean get() = serverURL.isNotBlank()

    /** Whether `other` points at another server, or at it with another token. */
    fun sameServer(other: ServerSettings): Boolean =
        serverURL.trim() == other.serverURL.trim() && accessToken.trim() == other.accessToken.trim()

    /** Why the app can't use `serverURL`, or null; for the app to word in the user's language. */
    enum class UrlProblem { NOT_CONFIGURED, INVALID, NOT_HTTPS }

    /**
     * Why the app can't use `serverURL`, or null. Like the iOS app under App Transport Security, it
     * connects over HTTPS only (Android's default network security config allows no cleartext).
     */
    fun urlIssue(): UrlProblem? {
        val text = serverURL.trim()
        if (text.isEmpty()) return UrlProblem.NOT_CONFIGURED
        val uri = try {
            URI(text)
        } catch (_: Exception) {
            return UrlProblem.INVALID
        }
        return when {
            uri.scheme?.lowercase() == "http" -> UrlProblem.NOT_HTTPS
            uri.scheme?.lowercase() != "https" || uri.host.isNullOrEmpty() -> UrlProblem.INVALID
            else -> null
        }
    }

    /** [urlIssue] in English. */
    fun urlProblem(): String? = when (urlIssue()) {
        UrlProblem.NOT_CONFIGURED -> ChatException.NotConfigured.message
        UrlProblem.INVALID -> ChatException.InvalidURL.message
        UrlProblem.NOT_HTTPS -> HTTPS_ONLY
        null -> null
    }

    companion object {
        const val HTTPS_ONLY = "Cedar connects over HTTPS only. Use the chat's https:// address, such as the one " +
            "tailscale serve gives it: https://<machine>.<tailnet>.ts.net (no port)."
    }
}

/**
 * The plan modes a message can ask for, with their labels: Cedar decides (`tool`, it proposes a
 * plan with `propose_plan` when a request needs one), for longer requests, always, or never.
 * The empty mode leaves it to the server's Settings.
 */
val PLAN_MODES: List<Pair<String, String>> = listOf(
    "" to "Server setting",
    "tool" to "When Cedar thinks it helps",
    "auto" to "For longer requests",
    "on" to "Always",
    "off" to "Never",
)
