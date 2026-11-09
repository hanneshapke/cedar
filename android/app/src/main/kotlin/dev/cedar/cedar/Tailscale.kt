package dev.cedar.cedar

import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import dev.cedar.core.ServerSettings

/**
 * The Tailscale app, which the phone needs to reach a server served with `tailscale serve`. Seeing
 * it from Android 11 on takes the `<queries>` entry in the manifest.
 */
object TailscaleApp {
    fun isInstalled(context: Context): Boolean = try {
        val pm = context.packageManager
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageInfo(ServerSettings.TAILSCALE_PACKAGE, PackageManager.PackageInfoFlags.of(0))
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageInfo(ServerSettings.TAILSCALE_PACKAGE, 0)
        }
        true
    } catch (_: PackageManager.NameNotFoundException) {
        false
    }

    /** Its page in the phone's app store, or on Google Play in the browser when there is none. */
    fun openStore(context: Context) {
        val id = ServerSettings.TAILSCALE_PACKAGE
        val market = Intent(Intent.ACTION_VIEW, Uri.parse("market://details?id=$id"))
        try {
            context.startActivity(market)
        } catch (_: ActivityNotFoundException) {
            try {
                context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse("https://play.google.com/store/apps/details?id=$id")))
            } catch (_: ActivityNotFoundException) {
            }
        }
    }
}
