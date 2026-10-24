package dev.cedar.core

import java.net.URI
import java.net.URLDecoder

/**
 * What the pairing QR code carries: `cedar://connect?server=<url>&token=<token>`, the chat's HTTPS
 * address on the tailnet and its access token (`pairing/src/connect.rs` makes it, for the chat's
 * Settings → Phone page, the desktop app and `scripts/cedar-stack.sh --pair`). The iOS app reads the
 * same link.
 */
data class ConnectionCode(val serverURL: String, val accessToken: String) {
    companion object {
        /**
         * `null` for anything else a camera might see, and for an address the app couldn't use anyway
         * (it connects over HTTPS only).
         */
        fun parse(text: String): ConnectionCode? {
            val uri = try {
                URI(text.trim())
            } catch (_: Exception) {
                return null
            }
            if (uri.scheme?.lowercase() != "cedar" || uri.host?.lowercase() != "connect") return null
            val items = (uri.rawQuery ?: "").split("&").filter { it.isNotEmpty() }.map { pair ->
                val eq = pair.indexOf('=')
                val name = decode(if (eq < 0) pair else pair.substring(0, eq))
                val value = if (eq < 0) "" else decode(pair.substring(eq + 1))
                name to value
            }
            fun value(name: String) = (items.firstOrNull { it.first == name }?.second ?: "").trim()
            val server = value("server")
            val url = try {
                URI(server)
            } catch (_: Exception) {
                return null
            }
            if (url.scheme?.lowercase() != "https" || url.host.isNullOrEmpty()) return null
            return ConnectionCode(server, value("token"))
        }

        // pairing/src/connect.rs form-encodes the query (`url`'s `query_pairs_mut`): a space is `+`,
        // a plus `%2B`.
        private fun decode(s: String): String = try {
            URLDecoder.decode(s, "UTF-8")
        } catch (_: IllegalArgumentException) {
            s
        }
    }
}
