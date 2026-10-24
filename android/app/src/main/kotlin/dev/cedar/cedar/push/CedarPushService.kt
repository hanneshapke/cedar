package dev.cedar.cedar.push

import android.util.Log
import dev.cedar.cedar.CedarApplication
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage
import kotlin.concurrent.thread

/**
 * UnifiedPush's side (ntfy or another distributor): the endpoint goes to the server, a message
 * becomes a notification. The callbacks come on the main thread.
 */
class CedarPushService : PushService() {
    private val push get() = (application as CedarApplication).push

    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        // cedar-chat only pushes to https endpoints, as ntfy.sh and a self-hosted ntfy behind TLS give.
        if (!endpoint.url.startsWith("https://")) {
            Log.w("CedarPush", "ignoring a non-https UnifiedPush endpoint")
            return
        }
        push.onToken(Transport.UNIFIED_PUSH, endpoint.url)
    }

    override fun onMessage(message: PushMessage, instance: String) {
        val data = Notifier.parse(message.content)
        // Fetching the conversation is network work: off the main thread.
        thread(name = "cedar-push") { Notifier.handle(applicationContext, data) }
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        Log.w("CedarPush", "UnifiedPush registration failed: $reason")
    }

    override fun onUnregistered(instance: String) {
        push.onUnregistered()
    }
}
