package dev.cedar.cedar

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.core.content.ContextCompat
import dev.cedar.cedar.push.Notifier
import dev.cedar.cedar.ui.CedarTheme
import dev.cedar.cedar.ui.RootScreen
import dev.cedar.core.ConnectionCode

class MainActivity : ComponentActivity() {
    private val app get() = (application as CedarApplication).app
    private val push get() = (application as CedarApplication).push

    private val askNotifications = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) push.register()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        setContent {
            CedarTheme {
                RootScreen(app, onAllowNotifications = ::allowNotifications)
            }
        }
        handle(intent)
        if (app.settings.isConfigured) allowNotifications()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    /** A tapped notification opens its conversation; a `cedar://connect` link fills in Settings. */
    private fun handle(intent: Intent?) {
        intent ?: return
        intent.getStringExtra(Notifier.EXTRA_CONVERSATION)?.let { app.open(it) }
        val link = intent.dataString
        if (intent.action == Intent.ACTION_VIEW && link != null) {
            ConnectionCode.parse(link)?.let { app.pendingConnection = it; app.showingSettings = true }
        }
    }

    /** Asks for the notification permission (Android 13+), then registers for push. */
    fun allowNotifications() {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            askNotifications.launch(Manifest.permission.POST_NOTIFICATIONS)
        } else {
            push.register()
        }
    }
}
