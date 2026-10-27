package dev.cedar.cedar

import android.app.Application
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner
import dev.cedar.cedar.push.Notifier
import dev.cedar.cedar.push.PushManager
import dev.cedar.cedar.storage.SettingsStore
import dev.cedar.core.ChatClient
import dev.cedar.core.ChatException
import org.osmdroid.config.Configuration

/** Holds the one [AppModel] for as long as the process lives, and follows the app to the front and back. */
class CedarApplication : Application() {
    lateinit var app: AppModel
        private set
    lateinit var push: PushManager
        private set

    override fun onCreate() {
        super.onCreate()
        val store = SettingsStore(this)
        push = PushManager(this, store) { settings ->
            try {
                ChatClient.from(settings ?: app.settings)
            } catch (_: ChatException) {
                null
            }
        }
        app = AppModel(this, store, push)
        // Numbers and dates in the user's language from the start, for notifications too.
        Language.setDefault(store.language)
        Notifier.createChannel(app.text())
        // OpenStreetMap's tile policy asks for an identifying user agent; tiles are cached in the
        // app's own cache, which needs no storage permission.
        Configuration.getInstance().apply {
            load(this@CedarApplication, getSharedPreferences("osmdroid", MODE_PRIVATE))
            userAgentValue = "$packageName/${BuildConfig.VERSION_NAME}"
            osmdroidBasePath = java.io.File(cacheDir, "osmdroid")
            osmdroidTileCache = java.io.File(cacheDir, "osmdroid/tiles")
        }

        ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
            override fun onStart(owner: LifecycleOwner) = app.becameActive()
            override fun onStop(owner: LifecycleOwner) = app.becameInactive()
        })
    }
}
