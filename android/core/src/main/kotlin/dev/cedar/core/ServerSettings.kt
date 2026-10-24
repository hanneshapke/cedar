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
) {
    val isConfigured: Boolean get() = serverURL.isNotBlank()

    /** Whether `other` points at another server, or at it with another token. */
    fun sameServer(other: ServerSettings): Boolean =
        serverURL.trim() == other.serverURL.trim() && accessToken.trim() == other.accessToken.trim()

    /**
     * Why the app can't use `serverURL`, or null. Like the iOS app under App Transport Security, it
     * connects over HTTPS only (Android's default network security config allows no cleartext).
     */
    fun urlProblem(): String? {
        val text = serverURL.trim()
        if (text.isEmpty()) return ChatException.NotConfigured.message
        val uri = try {
            URI(text)
        } catch (_: Exception) {
            return ChatException.InvalidURL.message
        }
        return when {
            uri.scheme?.lowercase() == "http" -> HTTPS_ONLY
            uri.scheme?.lowercase() != "https" || uri.host.isNullOrEmpty() -> ChatException.InvalidURL.message
            else -> null
        }
    }

    companion object {
        const val HTTPS_ONLY = "Cedar connects over HTTPS only. Use the chat's https:// address, such as the one " +
            "tailscale serve gives it: https://<machine>.<tailnet>.ts.net (no port)."
    }
}
