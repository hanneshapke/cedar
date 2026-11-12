package dev.cedar.cedar

import android.content.Intent
import android.os.Bundle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AppCompatActivity
import dev.cedar.cedar.push.Notifier
import dev.cedar.cedar.ui.CedarTheme
import dev.cedar.cedar.ui.ShareScreen
import dev.cedar.core.DeviceRequest

/**
 * Cedar in the share sheet: takes a text or a link another app shares, asks which agent should
 * get it, and hands it over (`POST /api/agents/{id}/trigger`, `source: "share"`). It is a screen
 * of its own, over the app that shared, so the main activity stays where it was; opening the run
 * that started goes there.
 */
class ShareActivity : AppCompatActivity() {
    private val app get() = (application as CedarApplication).app

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        Language.setDefault(app.language)
        enableEdgeToEdge()
        val shared = shared(intent)
        setContent {
            CedarTheme {
                ShareScreen(
                    app, shared,
                    onOpenRun = { conversationId -> openCedar(conversationId) },
                    onOpenCedar = { openCedar(null) },
                    onClose = ::finish,
                )
            }
        }
    }

    /** What was shared: the text, its subject as the title, and the first link in it. `null` for anything but text. */
    private fun shared(intent: Intent?): DeviceRequest? {
        if (intent?.action != Intent.ACTION_SEND) return null
        // Extras another app packed can be anything, a class this app can't read among them.
        val text = runCatching { intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString() }.getOrNull()
        val subject = runCatching { intent.getStringExtra(Intent.EXTRA_SUBJECT) }.getOrNull()
        return DeviceRequest.shared(text, subject).takeIf { it.text != null || it.url != null }
    }

    /** Goes to the app itself, at the run's conversation if there is one, and leaves the share sheet's screen. */
    private fun openCedar(conversationId: String?) {
        startActivity(
            Intent(this, MainActivity::class.java).apply {
                flags = Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP
                if (conversationId != null) putExtra(Notifier.EXTRA_CONVERSATION, conversationId)
            },
        )
        finish()
    }
}
