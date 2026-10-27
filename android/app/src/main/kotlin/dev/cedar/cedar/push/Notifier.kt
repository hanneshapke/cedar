package dev.cedar.cedar.push

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import dev.cedar.cedar.CedarApplication
import dev.cedar.cedar.MainActivity
import dev.cedar.cedar.R
import dev.cedar.core.TurnStatus
import dev.cedar.core.get
import dev.cedar.core.parseJson
import dev.cedar.core.str
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Shows what an Android push stands for. The push itself carries only `{"kind", "conversation_id",
 * "tag"}`, so neither Google nor an ntfy server sees what was said; the title and text come from
 * cedar-chat, fetched here with the access token.
 */
object Notifier {
    const val CHANNEL = "responses"
    const val EXTRA_CONVERSATION = "conversation_id"

    /** Creates the channel, or renames it in the app's language (`context` should be localized). */
    fun createChannel(context: Context) {
        val channel = NotificationChannel(CHANNEL, context.getString(R.string.channel_responses), NotificationManager.IMPORTANCE_DEFAULT)
        channel.description = context.getString(R.string.channel_responses_description)
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
    }

    /** Handles a push's data, from FCM's map or UnifiedPush's JSON body. Runs on a background thread. */
    fun handle(context: Context, data: Map<String, String>) {
        val app = (context.applicationContext as CedarApplication).app
        val text = runOnMain { app.text() }
        val kind = data["kind"] ?: "turn"
        val conversationId = data["conversation_id"].orEmpty()
        val tag = data["tag"] ?: conversationId
        if (kind == "test" || conversationId.isEmpty()) {
            show(context, "test", null, text.getString(R.string.notification_test_title), text.getString(R.string.notification_test_text))
            return
        }
        // A response in the conversation that's open is already in front of the user.
        if (runOnMain { app.isShowing(conversationId) }) {
            runOnMain { app.markSeen(conversationId) }
            return
        }
        val client = app.client
        val detail = client?.let { runBlocking { withTimeoutOrNull(8_000) { runCatching { it.conversation(conversationId) }.getOrNull() } } }
        // Looked at on another device in the meantime: nothing to tell.
        if (detail != null && !detail.conversation.unread && !detail.conversation.needsAnswer) return
        val title = detail?.conversation?.titleOr(text.getString(R.string.new_chat)) ?: text.getString(R.string.app_name)
        val last = detail?.turns?.maxByOrNull { it.seq }
        val finished = text.getString(R.string.notification_finished)
        val body = when {
            last == null -> finished
            last.needsAnswer -> text.getString(R.string.notification_needs_answer, last.finalText.orEmpty()).trim()
            last.status == TurnStatus.FAILED -> text.getString(R.string.notification_failed, last.error ?: text.getString(R.string.notification_run_failed))
            last.status == TurnStatus.CANCELLED -> text.getString(R.string.notification_stopped)
            else -> last.finalText?.takeIf { it.isNotBlank() } ?: finished
        }
        show(context, tag, conversationId, title, body.take(400))
        runOnMain { app.scope.launch { app.refresh() } }
    }

    /** Parses a UnifiedPush message body (the same fields as FCM's data, as JSON). */
    fun parse(body: ByteArray): Map<String, String> {
        val json = parseJson(body.toString(Charsets.UTF_8)) ?: return emptyMap()
        return listOf("kind", "conversation_id", "tag").mapNotNull { key -> json[key].str?.let { key to it } }.toMap()
    }

    private fun show(context: Context, tag: String, conversationId: String?, title: String, body: String) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) return
        val open = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
            if (conversationId != null) putExtra(EXTRA_CONVERSATION, conversationId)
        }
        val pending = PendingIntent.getActivity(context, tag.hashCode(), open, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val notification = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(body))
            .setAutoCancel(true)
            .setContentIntent(pending)
            .setGroup(conversationId ?: tag)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(tag, 1, notification)
        } catch (_: SecurityException) {
            // The permission was taken back meanwhile.
        }
    }

    private fun <T> runOnMain(block: () -> T): T = runBlocking(Dispatchers.Main.immediate) { block() }
}
