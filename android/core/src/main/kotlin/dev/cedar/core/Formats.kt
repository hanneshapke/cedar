package dev.cedar.core

import java.text.NumberFormat
import kotlin.math.abs

// The words and numbers the cards show, as the iOS card views write them (`TripStyle`, `StockFormat`).

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
        return if (meters >= 1000) "${CardParse.fixed(meters / 1000, 1)} km" else "${Math.round(meters)} m"
    }

    fun money(cost: TripCard.Cost): String {
        val symbol = mapOf("USD" to "$", "EUR" to "€", "GBP" to "£")[cost.currency] ?: "${cost.currency} "
        val low = number(cost.low, 0, 2)
        val high = number(cost.high, 0, 2)
        return if (cost.low == cost.high) "~$symbol$low" else "~$symbol$low–$high"
    }
}

object StockFormat {
    private val SYMBOLS = mapOf("USD" to "$", "EUR" to "€", "GBP" to "£", "JPY" to "¥", "INR" to "₹", "CNY" to "¥", "KRW" to "₩")

    fun currencySymbol(currency: String?): String = currency?.let { SYMBOLS[it] } ?: currency ?: "Price"

    /** `x` in `currency`, with as many decimals as a price its size needs. */
    fun money(x: Double, currency: String?): String {
        val digits = if (abs(x) >= 1000) 0 else if (abs(x) >= 1) 2 else 4
        val n = number(x, digits, digits)
        val symbol = currency?.let { SYMBOLS[it] }
        return when {
            symbol != null -> "$symbol$n"
            currency != null -> "$n $currency"
            else -> n
        }
    }

    fun signed(x: Double?, digits: Int = 1): String {
        if (x == null) return "–"
        val sign = if (x > 0) "+" else if (x < 0) "−" else ""
        return "$sign${CardParse.fixed(abs(x), digits)}%"
    }
}

/** `v` with between `min` and `max` fraction digits, grouped as the device's locale writes numbers. */
fun number(v: Double, min: Int, max: Int): String {
    val f = NumberFormat.getNumberInstance()
    f.minimumFractionDigits = min
    f.maximumFractionDigits = max
    return f.format(v)
}
