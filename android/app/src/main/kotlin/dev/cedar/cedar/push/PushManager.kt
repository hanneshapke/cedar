package dev.cedar.cedar.push

import android.content.Context
import android.os.Build
import android.provider.Settings
import android.util.Log
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import com.google.android.gms.common.ConnectionResult
import com.google.android.gms.common.GoogleApiAvailability
import com.google.firebase.FirebaseApp
import com.google.firebase.messaging.FirebaseMessaging
import dev.cedar.cedar.BuildConfig
import dev.cedar.cedar.Language
import dev.cedar.cedar.R
import dev.cedar.cedar.storage.SettingsStore
import dev.cedar.cedar.ui.describe
import dev.cedar.core.ChatClient
import dev.cedar.core.ChatException
import dev.cedar.core.ServerSettings
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import org.unifiedpush.android.connector.UnifiedPush

/** How a push reaches this phone: Google's FCM, or a UnifiedPush distributor such as ntfy. */
enum class Transport(val id: String, val label: String) {
    FCM("fcm", "Google (FCM)"),
    UNIFIED_PUSH("unifiedpush", "ntfy / UnifiedPush");

    companion object {
        fun of(id: String?): Transport? = entries.firstOrNull { it.id == id }
    }
}

/**
 * Registers this phone for Android pushes from cedar-chat (Settings → Notifications → Android push
 * on the server), the way iOS/Cedar/App/PushRegistrar.swift does for APNs: it gets a token (FCM) or
 * an endpoint (UnifiedPush), hands it to the server, and moves the registration when the server or
 * the transport changes. Either way a push only says that a response ended in a conversation; the
 * app fetches the rest from cedar-chat itself ([Notifier]).
 */
class PushManager(private val context: Context, private val store: SettingsStore, private val clientFor: (ServerSettings?) -> ChatClient?) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    /** A line for Settings: what happened with the last registration. */
    var status by mutableStateOf<String?>(null)
        private set

    /** Bumped when the transport or the distributors change, so Settings redraws. */
    var revision by mutableStateOf(0)
        private set

    /** The app's text in its language (the status lines are made outside an activity). */
    private fun text(id: Int, vararg args: Any): String = Language.localized(context, store.language).getString(id, *args)

    /** FCM works when the build carries a Firebase project and the phone has Google Play services. */
    val fcmAvailable: Boolean
        get() = BuildConfig.HAS_FIREBASE && FirebaseApp.getApps(context).isNotEmpty() &&
            GoogleApiAvailability.getInstance().isGooglePlayServicesAvailable(context) == ConnectionResult.SUCCESS

    /** The UnifiedPush distributors installed (ntfy, …), by package name. */
    val distributors: List<String>
        get() = UnifiedPush.getDistributors(context).filter { it != context.packageName }

    val savedDistributor: String?
        get() = UnifiedPush.getAckDistributor(context) ?: UnifiedPush.getSavedDistributor(context)

    /**
     * The transport in use: the one picked in Settings, else UnifiedPush when a distributor was chosen,
     * else FCM when it works here, else UnifiedPush (which needs ntfy installed).
     */
    val transport: Transport
        get() {
            Transport.of(store.string(CHOSEN))?.let { if (it != Transport.FCM || fcmAvailable) return it }
            return when {
                savedDistributor != null -> Transport.UNIFIED_PUSH
                fcmAvailable -> Transport.FCM
                else -> Transport.UNIFIED_PUSH
            }
        }

    /** Registers with the current server over the current transport. Called on start and after the permission is granted. */
    fun register() {
        if (clientFor(null) == null) return
        when (transport) {
            Transport.FCM -> registerFcm()
            Transport.UNIFIED_PUSH -> registerUnifiedPush()
        }
    }

    private fun registerFcm() {
        if (!fcmAvailable) {
            status = text(R.string.push_fcm_missing)
            return
        }
        FirebaseMessaging.getInstance().token.addOnCompleteListener { task ->
            val token = if (task.isSuccessful) task.result else null
            if (token != null) {
                onToken(Transport.FCM, token)
            } else {
                status = text(R.string.push_fcm_no_token, task.exception?.message ?: text(R.string.unknown_error))
            }
        }
    }

    private fun registerUnifiedPush() {
        val saved = savedDistributor
        val installed = distributors
        val distributor = saved ?: installed.singleOrNull()
        if (distributor == null) {
            status = if (installed.isEmpty()) text(R.string.push_install_distributor) else text(R.string.push_choose_distributor)
            return
        }
        if (saved == null) UnifiedPush.saveDistributor(context, distributor)
        // The endpoint comes back to CedarPushService.onNewEndpoint.
        UnifiedPush.register(context, messageForDistributor = "Cedar")
    }

    /** Settings picked a distributor. */
    fun useDistributor(packageName: String) {
        UnifiedPush.saveDistributor(context, packageName)
        store.setString(CHOSEN, Transport.UNIFIED_PUSH.id)
        revision++
        switchTo(Transport.UNIFIED_PUSH)
    }

    /** Settings switched the transport: the old registration goes, from the server and the distributor. */
    fun switchTo(new: Transport) {
        store.setString(CHOSEN, new.id)
        revision++
        val oldToken = store.string(TOKEN)
        val oldTransport = Transport.of(store.string(TOKEN_TRANSPORT))
        if (oldTransport != null && oldTransport != new) {
            if (oldToken != null) unregisterFromServer(clientFor(null), oldToken)
            if (oldTransport == Transport.UNIFIED_PUSH) UnifiedPush.unregister(context)
            if (oldTransport == Transport.FCM && fcmAvailable) FirebaseMessaging.getInstance().deleteToken()
            store.setString(TOKEN, null)
            store.setString(TOKEN_TRANSPORT, null)
        }
        register()
    }

    /** A new FCM token or UnifiedPush endpoint: tell the server. */
    fun onToken(via: Transport, token: String) {
        if (via != transport) return
        val previous = store.string(TOKEN)
        store.setString(TOKEN, token)
        store.setString(TOKEN_TRANSPORT, via.id)
        val client = clientFor(null) ?: return
        scope.launch {
            try {
                if (previous != null && previous != token) runCatching { client.unregisterAndroidDevice(previous) }
                client.registerAndroidDevice(via.id, token, deviceName())
                store.setString(REGISTERED_SERVER, client.baseURL.toString())
                status = text(R.string.push_registered, via.label)
            } catch (e: ChatException) {
                status = text(R.string.push_register_failed, Language.localized(context, store.language).describe(e))
            } catch (e: Exception) {
                status = text(R.string.push_register_failed, e.message ?: e.toString())
                Log.w(TAG, "push registration failed", e)
            }
        }
    }

    /** The distributor dropped this app's registration (ntfy uninstalled, say). */
    fun onUnregistered() {
        val token = store.string(TOKEN)
        if (Transport.of(store.string(TOKEN_TRANSPORT)) == Transport.UNIFIED_PUSH && token != null) {
            unregisterFromServer(clientFor(null), token)
            store.setString(TOKEN, null)
            store.setString(TOKEN_TRANSPORT, null)
        }
        status = text(R.string.push_unregistered)
        revision++
    }

    /** The server (or its token) changed: take the registration along. */
    fun serverChanged(old: ServerSettings, new: ServerSettings) {
        val token = store.string(TOKEN)
        val oldClient = clientFor(old)
        if (token != null && oldClient != null && store.string(REGISTERED_SERVER) == oldClient.baseURL.toString()) {
            unregisterFromServer(oldClient, token)
            store.setString(REGISTERED_SERVER, null)
        }
        if (new.isConfigured) register()
    }

    /** Asks the server for a test notification through Android push. */
    suspend fun sendTest(): String {
        val client = clientFor(null) ?: throw ChatException.NotConfigured
        client.testNotification("android")
        return text(R.string.push_test_sent)
    }

    private fun unregisterFromServer(client: ChatClient?, token: String) {
        client ?: return
        scope.launch { runCatching { client.unregisterAndroidDevice(token) } }
    }

    private fun deviceName(): String =
        Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)?.takeIf { it.isNotBlank() }
            ?: "${Build.MANUFACTURER.replaceFirstChar { it.uppercase() }} ${Build.MODEL}"

    private companion object {
        const val TAG = "CedarPush"
        const val CHOSEN = "pushTransport"
        const val TOKEN = "pushToken"
        const val TOKEN_TRANSPORT = "pushTokenTransport"
        const val REGISTERED_SERVER = "pushRegisteredServer"
    }
}
