package dev.cedar.cedar.push

import com.google.firebase.messaging.FirebaseMessagingService
import com.google.firebase.messaging.RemoteMessage
import dev.cedar.cedar.CedarApplication
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.runBlocking

/** FCM's side: a new token goes to the server, a data message becomes a notification. */
class CedarFirebaseService : FirebaseMessagingService() {
    override fun onNewToken(token: String) {
        val push = (application as CedarApplication).push
        runBlocking(Dispatchers.Main.immediate) { push.onToken(Transport.FCM, token) }
    }

    // Called on a background thread, with about ten seconds to show something.
    override fun onMessageReceived(message: RemoteMessage) {
        Notifier.handle(applicationContext, message.data)
    }
}
