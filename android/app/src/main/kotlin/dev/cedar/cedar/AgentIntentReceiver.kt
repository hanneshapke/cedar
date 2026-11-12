package dev.cedar.cedar

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.widget.Toast
import dev.cedar.cedar.ui.describe
import dev.cedar.cedar.ui.describeHeld
import dev.cedar.core.DeviceRequest
import dev.cedar.core.Trigger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout

/**
 * Lets another app on the phone (Tasker, an automation app) start an agent: a broadcast with the
 * action [ACTION] and the extras `agent_id` and, if it likes, `text`, sent to this app's package.
 * The agent is asked for a run with `POST /api/agents/{id}/trigger`, `source: "automation"`.
 *
 * Any app on the phone can send this, so it does little and gives nothing away. It is refused
 * unless the user turned on "Allow other apps to start agents" (off by default); it starts only
 * agents made to be started from the device (the `device` trigger, which run with a careful
 * choice of tools, because the text is whatever the other app says); the id can only be an id;
 * and the sender learns one word about how it went ([RESULT_STARTED] and the like, as the result
 * of an ordered broadcast), never the server's address, the token or anything the server said.
 */
class AgentIntentReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != ACTION) return
        val app = (context.applicationContext as CedarApplication).app
        val words = app.text()
        val ordered = isOrderedBroadcast

        /** Says how it went: to the sender in a word, to the user in a line. */
        fun refuse(result: String, message: String) {
            if (ordered) setResult(Activity.RESULT_CANCELED, result, null)
            Toast.makeText(context, message, Toast.LENGTH_LONG).show()
        }

        if (!app.allowAutomation) return refuse(RESULT_OFF, words.getString(R.string.automation_off))
        // Extras another app packed can be anything, a class this app can't read among them.
        val id = DeviceRequest.agentId(runCatching { intent.getStringExtra(EXTRA_AGENT_ID) }.getOrNull())
            ?: return refuse(RESULT_NO_AGENT, words.getString(R.string.automation_no_agent))
        val text = runCatching { intent.getStringExtra(EXTRA_TEXT) }.getOrNull()?.trim()?.takeIf { it.isNotEmpty() }
        if (text != null && text.toByteArray().size > DeviceRequest.MAX_BYTES) {
            return refuse(RESULT_TOO_LARGE, words.getString(R.string.automation_too_large))
        }
        val client = app.client ?: return refuse(RESULT_NOT_CONNECTED, words.getString(R.string.error_not_configured))

        // The server is asked off the main thread; the broadcast stays open until it has answered.
        // A foreground broadcast has ten seconds for that, any other a minute.
        val pending = goAsync()
        val foreground = intent.flags and Intent.FLAG_RECEIVER_FOREGROUND != 0
        scope.launch {
            var result = RESULT_ERROR
            val message = try {
                withTimeout(if (foreground) 8_000 else 25_000) {
                    val agent = client.agent(id).agent
                    if (agent.trigger !is Trigger.Device) {
                        result = RESULT_NOT_DEVICE
                        return@withTimeout words.getString(R.string.automation_not_device, agent.title)
                    }
                    val start = client.triggerAgent(id, source = SOURCE, text = text)
                    if (start.started) {
                        result = RESULT_STARTED
                        words.getString(R.string.automation_started, agent.title)
                    } else {
                        result = RESULT_HELD + start.held.orEmpty().filter { it in 'a'..'z' || it == '_' }.take(30)
                        words.getString(R.string.automation_not_started, agent.title, words.describeHeld(start.held))
                    }
                }
            } catch (_: TimeoutCancellationException) {
                words.getString(R.string.automation_failed, words.getString(R.string.automation_timeout))
            } catch (e: Exception) {
                words.getString(R.string.automation_failed, words.describe(e))
            }
            try {
                withContext(Dispatchers.Main) { Toast.makeText(context, message, Toast.LENGTH_LONG).show() }
                if (ordered) pending.setResult(if (result == RESULT_STARTED) Activity.RESULT_OK else Activity.RESULT_CANCELED, result, null)
            } finally {
                pending.finish()
            }
        }
    }

    companion object {
        /** The action another app sends, as a broadcast to this app's package. */
        const val ACTION = "dev.cedar.action.RUN_AGENT"

        /** Required: the agent's id, as its page shows it. */
        const val EXTRA_AGENT_ID = "agent_id"

        /** Optional: a text the run is handed as data. */
        const val EXTRA_TEXT = "text"

        /** What the server is told the request comes from. */
        const val SOURCE = "automation"

        // The result data of an ordered broadcast: how it went, in a word.
        const val RESULT_STARTED = "started"

        /** Followed by the server's reason: `held:too_soon`. */
        const val RESULT_HELD = "held:"
        const val RESULT_OFF = "refused:off"
        const val RESULT_NO_AGENT = "refused:agent_id"
        const val RESULT_NOT_DEVICE = "refused:not_a_device_agent"
        const val RESULT_TOO_LARGE = "refused:too_large"
        const val RESULT_NOT_CONNECTED = "refused:not_connected"
        const val RESULT_ERROR = "error"

        private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    }
}
