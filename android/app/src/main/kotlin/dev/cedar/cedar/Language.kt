package dev.cedar.cedar

import android.content.Context
import android.content.res.Configuration
import android.content.res.Resources
import android.os.LocaleList
import androidx.appcompat.app.AppCompatDelegate
import androidx.core.os.LocaleListCompat
import dev.cedar.core.AppLocales
import java.util.Locale

/**
 * The app's language. The choice is a tag from [AppLocales.OPTIONS], or "" for Automatic (follow the
 * device). AppCompat applies it to the activities ([AppCompatDelegate.setApplicationLocales], which on
 * Android 13+ is the system's per-app language and below that is stored by AppCompat itself); numbers
 * and dates follow the default [Locale], which is set here; text shown from outside an activity
 * (notifications, push status) is read through [localized].
 */
object Language {
    /** The device's own locale, whatever the app is set to. */
    fun deviceLocale(): Locale = Resources.getSystem().configuration.locales.get(0) ?: Locale.getDefault()

    /** The locale the server is told and the app formats with ([AppLocales.effective]). */
    fun effective(choice: String?): String = AppLocales.effective(choice?.ifEmpty { null }, deviceLocale())

    /** Switches the app to `choice` (activities are recreated in it) and formats in it from now on. */
    fun apply(choice: String) {
        val locales = if (choice.isEmpty()) LocaleListCompat.getEmptyLocaleList() else LocaleListCompat.forLanguageTags(choice)
        AppCompatDelegate.setApplicationLocales(locales)
        setDefault(choice)
    }

    /** Numbers, dates and prices are written in the effective locale. */
    fun setDefault(choice: String?) {
        Locale.setDefault(Locale.forLanguageTag(effective(choice)))
    }

    /**
     * The choice the system holds for the app (Android 13+, where it can be changed in the system's
     * settings), as one of the app's options: "" for none, else the offered tag closest to it.
     */
    fun systemChoice(): String {
        val locale = AppCompatDelegate.getApplicationLocales()[0] ?: return ""
        return AppLocales.option(locale.toLanguageTag())?.tag ?: AppLocales.suggested(locale)
    }

    private var cached: Pair<String, Context>? = null

    /**
     * `base` with the app's language: itself for Automatic, else a context whose resources are in
     * `choice`. For text read outside an activity, whose resources AppCompat doesn't localize before
     * Android 13.
     */
    fun localized(base: Context, choice: String?): Context {
        val tag = choice?.takeIf { it.isNotEmpty() } ?: return base
        cached?.let { (t, context) -> if (t == tag) return context }
        val config = Configuration(base.resources.configuration).apply { setLocales(LocaleList.forLanguageTags(tag)) }
        return base.createConfigurationContext(config).also { cached = tag to it }
    }
}
