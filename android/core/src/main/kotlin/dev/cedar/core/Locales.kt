package dev.cedar.core

import java.util.Locale

/**
 * The languages the app speaks and the locale it tells cedar-chat about: with each message
 * (`"locale"` in `POST /api/conversations/{id}/messages`) and as the server's own setting
 * (`PUT /api/settings/locale`), so scheduled runs and notifications follow it too.
 *
 * A choice is a BCP 47 tag from [OPTIONS], or `null` for Automatic (follow the device).
 */
object AppLocales {
    /** A locale the user can pick, labelled in its own language. */
    data class Option(val tag: String, val label: String) {
        val language: String get() = tag.substringBefore('-')
    }

    val OPTIONS: List<Option> = listOf(
        Option("en", "English"),
        Option("en-US", "English (United States)"),
        Option("en-GB", "English (United Kingdom)"),
        Option("de-DE", "Deutsch (Deutschland)"),
        Option("de-AT", "Deutsch (Österreich)"),
        Option("de-CH", "Deutsch (Schweiz)"),
        Option("fr-FR", "Français (France)"),
        Option("fr-BE", "Français (Belgique)"),
        Option("fr-CH", "Français (Suisse)"),
        Option("fr-CA", "Français (Canada)"),
        Option("es-ES", "Español (España)"),
        Option("es-MX", "Español (México)"),
        Option("es-419", "Español (Latinoamérica)"),
        Option("es-US", "Español (Estados Unidos)"),
    )

    /** The languages the app is translated into; English is the source and the fallback. */
    val LANGUAGES: Set<String> = setOf("en", "de", "fr", "es")
    const val FALLBACK = "en"

    /**
     * `tag` as a language and an optional region with their usual case (`de_ch` and `DE-CH` are
     * `de-CH`; a numeric region stays, so `es_419` is `es-419`), or `null` when it is empty or names
     * no language. Scripts and extensions are dropped.
     */
    fun normalize(tag: String?): String? {
        val raw = tag?.trim()?.replace('_', '-')?.takeIf { it.isNotEmpty() } ?: return null
        val locale = Locale.forLanguageTag(raw)
        val language = locale.language.takeIf { it.isNotEmpty() && it != "und" } ?: return null
        return if (locale.country.isEmpty()) language else "$language-${locale.country}"
    }

    /** The offered option `tag` names, or `null`. */
    fun option(tag: String?): Option? {
        val normalized = normalize(tag) ?: return null
        return OPTIONS.firstOrNull { it.tag == normalized }
    }

    /**
     * The option for a language's regions that aren't offered, where it isn't the language's first
     * option: Spanish outside Spain, Mexico and the US is Latin American Spanish.
     */
    private val OTHER_REGIONS: Map<String, String> = mapOf("es" to "es-419")

    /**
     * The option to preselect for someone who hasn't chosen yet: the device's locale when it is
     * offered, else the language's option for other regions ([OTHER_REGIONS]) or its first offered
     * one, else English.
     */
    fun suggested(device: Locale): String {
        val tag = normalize(device.toLanguageTag())
        OPTIONS.firstOrNull { it.tag == tag }?.let { return it.tag }
        val language = tag?.substringBefore('-')
        OTHER_REGIONS[language]?.let { return it }
        return OPTIONS.firstOrNull { it.language == language }?.tag ?: FALLBACK
    }

    /**
     * The locale the server is told, and the app formats with: the `choice`, or for Automatic the
     * device's locale (with its region) when its language is one the app speaks, else English.
     */
    fun effective(choice: String?, device: Locale): String {
        normalize(choice)?.let { return it }
        val tag = normalize(device.toLanguageTag()) ?: return FALLBACK
        return if (tag.substringBefore('-') in LANGUAGES) tag else FALLBACK
    }
}
