package dev.cedar.core

import java.text.DecimalFormat
import java.text.NumberFormat
import java.util.Currency
import java.util.Locale
import kotlin.math.abs

// The words and numbers the cards show, as the iOS card views write them (`TripStyle`, `StockFormat`).
// Numbers and prices are written as the default locale does, which the app sets to the user's
// language; words are English here (the app has its own, translated, for what it shows).

object TripStyle {
    fun duration(minutes: Int): String {
        if (minutes < 60) return "$minutes min"
        val h = minutes / 60
        val m = minutes % 60
        return if (m == 0) "$h h" else "$h h $m min"
    }

    fun distance(km: Double?, meters: Double?): String? {
        if (km != null) return "${number(km, 0, 1)} km"
        if (meters == null) return null
        return if (meters >= 1000) "${number(meters / 1000, 1, 1)} km" else "${number(meters, 0, 0)} m"
    }

    /** "~$15–22", "~15–22 €". */
    fun money(cost: TripCard.Cost): String =
        if (cost.low == cost.high) "~${price(cost.low, cost.currency, 0, 2)}" else "~${priceRange(cost.low, cost.high, cost.currency, 0, 2)}"
}

object StockFormat {
    private val SYMBOLS = mapOf("USD" to "$", "EUR" to "€", "GBP" to "£", "JPY" to "¥", "INR" to "₹", "CNY" to "¥", "KRW" to "₩")

    fun currencySymbol(currency: String?): String = currency?.let { SYMBOLS[it] } ?: currency ?: "Price"

    /** `x` in `currency`, with as many decimals as a price its size needs. */
    fun money(x: Double, currency: String?): String {
        val digits = if (abs(x) >= 1000) 0 else if (abs(x) >= 1) 2 else 4
        return if (currency == null) number(x, digits, digits) else price(x, currency, digits, digits)
    }

    /** A change in percent, always signed: "+1.2%", "−1,2 %". */
    fun signed(x: Double?, digits: Int = 1, locale: Locale = Locale.getDefault()): String {
        if (x == null) return "–"
        val sign = if (x > 0) "+" else if (x < 0) "−" else ""
        val f = NumberFormat.getPercentInstance(locale)
        f.minimumFractionDigits = digits
        f.maximumFractionDigits = digits
        return sign + f.format(abs(x) / 100)
    }
}

/** `v` with between `min` and `max` fraction digits, grouped as `locale` (the user's) writes numbers. */
fun number(v: Double, min: Int, max: Int, locale: Locale = Locale.getDefault()): String {
    val f = NumberFormat.getNumberInstance(locale)
    f.minimumFractionDigits = min
    f.maximumFractionDigits = max
    return f.format(v)
}

/** A price format for the three-letter `currency` in `locale`, or `null` for a code Java doesn't know. */
private fun currencyFormat(currency: String, min: Int, max: Int, locale: Locale): NumberFormat? {
    val code = try {
        Currency.getInstance(currency.uppercase())
    } catch (_: IllegalArgumentException) {
        return null
    }
    return NumberFormat.getCurrencyInstance(locale).apply {
        this.currency = code
        minimumFractionDigits = min
        maximumFractionDigits = max
    }
}

/** `v` in `currency`, as `locale` writes prices: "$15.00" in English, "15,00 $" in German. */
fun price(v: Double, currency: String, min: Int, max: Int, locale: Locale = Locale.getDefault()): String =
    currencyFormat(currency, min, max, locale)?.format(v) ?: "${number(v, min, max, locale)} $currency"

/** A range of prices with the currency once: "$15–22", "15–22 €". */
fun priceRange(low: Double, high: Double, currency: String, min: Int, max: Int, locale: Locale = Locale.getDefault()): String {
    val f = currencyFormat(currency, min, max, locale)
        ?: return "${number(low, min, max, locale)}–${number(high, min, max, locale)} $currency"
    val whole = f.format(high)
    // The same number without the currency's prefix and suffix, to put the range in its place.
    val bare = (f.clone() as? DecimalFormat)?.apply {
        positivePrefix = ""
        positiveSuffix = ""
    } ?: return "${f.format(low)}–$whole"
    val highText = bare.format(high)
    val at = whole.indexOf(highText)
    if (at < 0) return "${f.format(low)}–$whole"
    return whole.substring(0, at) + bare.format(low) + "–" + highText + whole.substring(at + highText.length)
}
