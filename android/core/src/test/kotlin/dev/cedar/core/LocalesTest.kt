package dev.cedar.core

import java.util.Locale
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** The locales the app offers, the one it tells the server, and the numbers and prices it writes in them. */
class LocalesTest {
    @Test
    fun normalizesTags() {
        assertEquals("de-CH", AppLocales.normalize("de_ch"))
        assertEquals("de-CH", AppLocales.normalize(" DE-CH "))
        assertEquals("en", AppLocales.normalize("EN"))
        assertEquals("fr-CA", AppLocales.normalize("fr-Latn-CA"))
        // A UN M.49 region (Latin America) is kept as it is.
        assertEquals("es-419", AppLocales.normalize("es-419"))
        assertEquals("es-419", AppLocales.normalize("ES_419"))
        assertEquals("es-MX", AppLocales.normalize("es_mx"))
        assertNull(AppLocales.normalize(""))
        assertNull(AppLocales.normalize(null))
        assertNull(AppLocales.normalize("12"))
    }

    @Test
    fun offersEachLocaleInItsOwnLanguage() {
        assertEquals(
            listOf("en", "en-US", "en-GB", "de-DE", "de-AT", "de-CH", "fr-FR", "fr-BE", "fr-CH", "fr-CA",
                "es-ES", "es-MX", "es-419", "es-US"),
            AppLocales.OPTIONS.map { it.tag },
        )
        assertEquals("Deutsch (Österreich)", AppLocales.option("de_AT")?.label)
        assertEquals("Español (Latinoamérica)", AppLocales.option("es_419")?.label)
        assertEquals("Español (España)", AppLocales.option("es-ES")?.label)
        assertNull(AppLocales.option("es-AR"))
        assertNull(AppLocales.option("it-IT"))
        // Every tag is already in its normal form, so a stored choice reads back as itself.
        AppLocales.OPTIONS.forEach { assertEquals(it.tag, AppLocales.normalize(it.tag)) }
    }

    @Test
    fun suggestsTheDevicesLocaleWhenOffered() {
        assertEquals("de-CH", AppLocales.suggested(Locale.forLanguageTag("de-CH")))
        assertEquals("en-GB", AppLocales.suggested(Locale.UK))
        // Another region of one of the languages: its first locale.
        assertEquals("de-DE", AppLocales.suggested(Locale.forLanguageTag("de-LU")))
        assertEquals("fr-FR", AppLocales.suggested(Locale.forLanguageTag("fr")))
        assertEquals("en", AppLocales.suggested(Locale.forLanguageTag("en-AU")))
        assertEquals("es-ES", AppLocales.suggested(Locale.forLanguageTag("es-ES")))
        assertEquals("es-MX", AppLocales.suggested(Locale.forLanguageTag("es-MX")))
        assertEquals("es-US", AppLocales.suggested(Locale.forLanguageTag("es-US")))
        assertEquals("es-419", AppLocales.suggested(Locale.forLanguageTag("es-419")))
        // Spanish from anywhere else, or with no region, is Latin American Spanish.
        assertEquals("es-419", AppLocales.suggested(Locale.forLanguageTag("es-AR")))
        assertEquals("es-419", AppLocales.suggested(Locale.forLanguageTag("es-CO")))
        assertEquals("es-419", AppLocales.suggested(Locale.forLanguageTag("es")))
        assertEquals("en", AppLocales.suggested(Locale.forLanguageTag("it-IT")))
    }

    @Test
    fun theEffectiveLocaleIsTheChoiceOrTheDevicesInOneOfOurLanguages() {
        assertEquals("fr-BE", AppLocales.effective("fr-BE", Locale.GERMANY))
        assertEquals("de-CH", AppLocales.effective("de_CH", Locale.US))
        // Automatic: the device's locale, with its region, when the app speaks its language.
        assertEquals("de-AT", AppLocales.effective(null, Locale.forLanguageTag("de-AT")))
        assertEquals("de-LU", AppLocales.effective("", Locale.forLanguageTag("de-LU")))
        assertEquals("fr", AppLocales.effective(null, Locale.FRENCH))
        assertEquals("en-AU", AppLocales.effective(null, Locale.forLanguageTag("en-AU")))
        assertEquals("es-ES", AppLocales.effective(null, Locale.forLanguageTag("es-ES")))
        assertEquals("es-AR", AppLocales.effective(null, Locale.forLanguageTag("es-AR")))
        assertEquals("es-419", AppLocales.effective("es-419", Locale.GERMANY))
        assertEquals("en", AppLocales.effective(null, Locale.forLanguageTag("it-IT")))
        assertEquals("en", AppLocales.effective(null, Locale.ROOT))
    }

    @Test
    fun pricesAndNumbersFollowTheLocale() {
        val de = Locale.GERMANY
        assertEquals("1.234,5", number(1234.5, 1, 1, de))
        assertTrue(price(15.0, "EUR", 2, 2, de).matches(Regex("15,00[\\s\\u00A0\\u202F]€")), price(15.0, "EUR", 2, 2, de))
        assertEquals("$15.00", price(15.0, "USD", 2, 2, Locale.US))
        assertEquals("$15–22", priceRange(15.0, 22.0, "USD", 0, 2, Locale.US))
        assertTrue(priceRange(15.0, 22.0, "EUR", 0, 2, de).matches(Regex("15–22[\\s\\u00A0\\u202F]€")), priceRange(15.0, 22.0, "EUR", 0, 2, de))
        assertEquals("15–22 XYZ", priceRange(15.0, 22.0, "XYZ", 0, 2, Locale.US), "a code Java doesn't know goes after the number")
        assertEquals("−1.2%", StockFormat.signed(-1.234, locale = Locale.US))
        assertTrue(StockFormat.signed(1.234, locale = de).matches(Regex("\\+1,2[\\s\\u00A0\\u202F]%")), StockFormat.signed(1.234, locale = de))
    }
}
