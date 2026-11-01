package dev.cedar.core

/**
 * A `weather_forecast` result, read back from the plain-text report the model gets (see
 * src/tools/weather.rs, `format_forecast`), with the same rules as the web UI's
 * chat/static/weather-view.js and iOS/Cedar/Models/WeatherCard.swift, so the tool and what the model
 * sees are unchanged and older conversations get the card too:
 *
 *     Berlin, State of Berlin, Germany (52.52, 13.41), timezone Europe/Berlin
 *
 *     Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, wind 4.6 km/h from NW, ...
 *
 *     Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), ...
 *
 *     Weather data by Open-Meteo.com (CC BY 4.0)
 *
 * Any part the report leaves out is `null`. The text came from the network, so it is clipped and only
 * shown as text.
 */
data class WeatherCard(
    /** "Berlin, State of Berlin, Germany", or empty when the report only has coordinates. */
    val label: String,
    val latitude: Double,
    val longitude: Double,
    val timezone: String?,
    val now: Now?,
    val days: List<Day>,
) {
    /** A number with its unit: `10.2 °C`, `4.6 km/h`, `70 %`, or unitless for the UV index. */
    data class Quantity(val value: Double, val unit: String) {
        /** The temperature in °C whichever unit it came in, for picking a colour. */
        val celsius: Double get() = if (unit == "°F") (value - 32) * 5 / 9 else value

        companion object {
            /** "10.2°C", "4.6 km/h", "70%", "2.9"; `null` if `text` doesn't start with a number. */
            fun parse(text: String): Quantity? {
                val s = text.trim()
                val end = s.indexOfFirst { it !in "-0123456789.e" }.let { if (it < 0) s.length else it }
                if (end == 0) return null
                val value = s.substring(0, end).toDoubleOrNull()?.takeIf { it.isFinite() } ?: return null
                return Quantity(value, clip(s.substring(end)))
            }
        }
    }

    data class Now(
        val date: String? = null,
        /** "21:00", the report's local time. */
        val time: String? = null,
        val code: Int? = null,
        val temp: Quantity? = null,
        val feels: Quantity? = null,
        val humidity: Quantity? = null,
        val wind: Quantity? = null,
        /** A 16-point compass name the wind comes from. */
        val from: String? = null,
        val gusts: Quantity? = null,
        val precip: Quantity? = null,
    )

    data class Day(
        /** "2026-10-12" */
        val date: String,
        val code: Int? = null,
        val low: Quantity? = null,
        val high: Quantity? = null,
        val precip: Quantity? = null,
        val chance: Quantity? = null,
        val wind: Quantity? = null,
        val gusts: Quantity? = null,
        val uv: Quantity? = null,
        val sunrise: String? = null,
        val sunset: String? = null,
    )

    /** What each WMO code looks like; clear and partly cloudy have a night version. */
    enum class Sky {
        CLEAR, PARTLY, CLOUD, FOG, DRIZZLE, RAIN, SLEET, SNOW, STORM;

        companion object {
            fun of(code: Int?): Sky = when (code ?: -1) {
                0, 1 -> CLEAR
                2 -> PARTLY
                3 -> CLOUD
                45, 48 -> FOG
                in 51..57 -> DRIZZLE
                66, 67 -> SLEET
                in 61..65, in 80..82 -> RAIN
                in 71..77, 85, 86 -> SNOW
                in 95..99 -> STORM
                else -> CLOUD
            }
        }
    }

    // What the card shows

    /** The report's local date: the current conditions', else the first day's. */
    val today: String? get() = now?.date ?: days.firstOrNull()?.date

    val todayDay: Day? get() = days.firstOrNull { it.date == today }

    /** Night if the report's local time is outside today's sunrise to sunset. */
    val isNight: Boolean
        get() {
            val time = now?.time ?: return false
            val day = todayDay ?: return false
            val rise = day.sunrise ?: return false
            val set = day.sunset ?: return false
            return time < rise || time >= set
        }

    /** The condition the card leads with: now, else the first day. */
    val leadCode: Int? get() = now?.code ?: days.firstOrNull()?.code

    /** The place's name, and the rest of its label ("State of Berlin, Germany"). */
    val placeName: String
        get() = label.split(", ", limit = 2).first().ifEmpty { "${CardParse.fixed(latitude, 2)}, ${CardParse.fixed(longitude, 2)}" }

    val region: String? get() = label.split(", ").let { if (it.size > 1) it.drop(1).joinToString(", ") else null }

    /** The days with both a low and a high, which the forecast rows need. */
    val forecastDays: List<Day> get() = days.filter { it.low != null && it.high != null }

    companion object {
        const val MAX_DAYS = 16
        const val MAX_TEXT = 200
        val COMPASS = listOf("N", "NNE", "NE", "ENE", "E", "ESE", "SE", "SSE", "S", "SSW", "SW", "WSW", "W", "WNW", "NW", "NNW")

        /** The words weather.rs writes for each WMO weather code, read back to the code. */
        val CODES: Map<String, Int> = mapOf(
            "clear sky" to 0, "mainly clear" to 1, "partly cloudy" to 2, "overcast" to 3, "fog" to 45, "freezing fog" to 48,
            "light drizzle" to 51, "moderate drizzle" to 53, "dense drizzle" to 55, "light freezing drizzle" to 56,
            "dense freezing drizzle" to 57, "slight rain" to 61, "moderate rain" to 63, "heavy rain" to 65,
            "light freezing rain" to 66, "heavy freezing rain" to 67, "slight snow" to 71, "moderate snow" to 73,
            "heavy snow" to 75, "snow grains" to 77, "slight rain showers" to 80, "moderate rain showers" to 81,
            "violent rain showers" to 82, "slight snow showers" to 85, "heavy snow showers" to 86, "thunderstorm" to 95,
            "thunderstorm with slight hail" to 96, "thunderstorm with heavy hail" to 99,
        )

        /** The English words for a WMO code; empty for one the list doesn't have. */
        fun conditionWords(code: Int): String = CODES.entries.firstOrNull { it.value == code }?.key ?: ""

        /** The hue (0–360) for a temperature in °C: cold blue through mild green and yellow to hot red. */
        fun hue(celsius: Double): Double {
            val stops = listOf(-15.0 to 230.0, 0.0 to 200.0, 10.0 to 160.0, 18.0 to 95.0, 25.0 to 40.0, 35.0 to 5.0)
            if (celsius <= stops[0].first) return stops[0].second
            for (i in 1 until stops.size) {
                val (t1, h1) = stops[i]
                val (t0, h0) = stops[i - 1]
                if (celsius <= t1) return Math.round(h0 + (h1 - h0) * ((celsius - t0) / (t1 - t0))).toDouble()
            }
            return stops.last().second
        }

        /** The forecast in a `weather_forecast` result, or `null` when `output` isn't one. */
        fun parse(output: String): WeatherCard? {
            val lines = output.lineSequence().map { it.trim() }.filter { it.isNotEmpty() }.toList()
            val head = lines.firstOrNull()?.let(::parseHeader) ?: return null
            var now: Now? = null
            val days = mutableListOf<Day>()
            for (line in lines.drop(1)) {
                if (now == null && line.startsWith("Now")) {
                    now = parseNow(line)
                } else if (days.size < MAX_DAYS) {
                    parseDay(line)?.let(days::add)
                }
            }
            if (now?.temp == null && days.none { it.low != null || it.high != null }) return null
            return WeatherCard(head.label, head.latitude, head.longitude, head.timezone, now, days)
        }

        fun clip(s: String): String = s.trim().take(MAX_TEXT)

        private data class Header(val label: String, val latitude: Double, val longitude: Double, val timezone: String?)

        /** "Berlin, Germany (52.52, 13.41), timezone Europe/Berlin", or just "52.52, 13.41". */
        private fun parseHeader(line: String): Header? {
            var rest = line
            var timezone: String? = null
            val zoneAt = rest.indexOf(", timezone ")
            if (zoneAt >= 0) {
                val zone = rest.substring(zoneAt + ", timezone ".length)
                if (zone.isEmpty() || zone.any { it.isWhitespace() }) return null
                timezone = clip(zone)
                rest = rest.substring(0, zoneAt)
            }
            var label = ""
            var coordinates = rest
            val open = rest.lastIndexOf('(')
            if (rest.endsWith(")") && open >= 0) {
                label = clip(rest.substring(0, open))
                coordinates = rest.substring(open + 1, rest.length - 1)
            }
            val parts = coordinates.split(",", limit = 2).map { it.trim() }
            if (parts.size != 2) return null
            val lat = parts[0].toDoubleOrNull()?.takeIf { CardParse.isCoordinate(it, 90.0) } ?: return null
            val lon = parts[1].toDoubleOrNull()?.takeIf { CardParse.isCoordinate(it, 180.0) } ?: return null
            return Header(label, lat, lon, timezone)
        }

        /** The weather code for a condition in words ("partly cloudy", "weather code 4"), or `null`. */
        private fun codeOf(words: String): Int? =
            CODES[words] ?: words.removePrefix("weather code ").takeIf { it != words }?.toIntOrNull()

        private fun after(prefix: String, part: String): String? = if (part.startsWith(prefix)) part.substring(prefix.length) else null

        /** "2026-10-12" */
        fun isDate(s: String): Boolean = s.length == 10 && s.withIndex().all { (i, c) -> if (i == 4 || i == 7) c == '-' else c.isDigit() }

        /** "07:26" */
        fun isClock(s: String): Boolean = s.length == 5 && s.withIndex().all { (i, c) -> if (i == 2) c == ':' else c.isDigit() }

        /** "Now (2026-10-12 21:00): clear sky, 10.2°C (feels 8.3°C), humidity 70%, wind 4.6 km/h from NW, ..." */
        private fun parseNow(line: String): Now? {
            val colon = line.indexOf(": ").takeIf { it >= 0 } ?: return null
            var now = Now()
            val stamp = line.substring(3, colon)
            if (stamp.startsWith(" (") && stamp.endsWith(")")) {
                val parts = stamp.substring(2, stamp.length - 1).split(" ")
                if (parts.size == 2 && isDate(parts[0]) && isClock(parts[1])) now = now.copy(date = parts[0], time = parts[1])
            }
            for (part in line.substring(colon + 2).split(", ")) {
                val code = codeOf(part)
                val feelsAt = part.indexOf(" (feels ")
                if (code != null) {
                    if (now.code == null) now = now.copy(code = code)
                } else if (feelsAt >= 0 && part.endsWith(")")) {
                    now = now.copy(
                        temp = Quantity.parse(part.substring(0, feelsAt)),
                        feels = Quantity.parse(part.substring(feelsAt + " (feels ".length, part.length - 1)),
                    )
                } else {
                    after("humidity ", part)?.let { now = now.copy(humidity = Quantity.parse(it)) }
                        ?: after("wind ", part)?.let { rest ->
                            val fromAt = rest.indexOf(" from ")
                            val from = if (fromAt >= 0) rest.substring(fromAt + " from ".length) else null
                            now = if (from != null && from in COMPASS) now.copy(wind = Quantity.parse(rest.substring(0, fromAt)), from = from)
                            else now.copy(wind = Quantity.parse(rest))
                        }
                        ?: after("gusts ", part)?.let { now = now.copy(gusts = Quantity.parse(it)) }
                        ?: after("precipitation ", part)?.let { now = now.copy(precip = Quantity.parse(it)) }
                        ?: run { if ('°' in part && now.temp == null) now = now.copy(temp = Quantity.parse(part)) }
                }
            }
            return now
        }

        /**
         * "Mon 2026-10-12: overcast, low 6.8°C, high 13.7°C, precipitation 0 mm (32% chance), wind up to
         * 14.1 km/h (gusts 31 km/h), UV 2.9, sunrise 07:26, sunset 18:18"
         */
        private fun parseDay(line: String): Day? {
            val colon = line.indexOf(": ").takeIf { it >= 0 } ?: return null
            val head = line.substring(0, colon).split(" ")
            if (head.size != 2 || head[0].length != 3 || !head[0][0].isUpperCase() || !isDate(head[1])) return null
            var day = Day(date = head[1])
            for (part in line.substring(colon + 2).split(", ")) {
                val code = codeOf(part)
                if (code != null) {
                    if (day.code == null) day = day.copy(code = code)
                    continue
                }
                after("low ", part)?.let { day = day.copy(low = Quantity.parse(it)) }
                    ?: after("high ", part)?.let { day = day.copy(high = Quantity.parse(it)) }
                    ?: after("precipitation ", part)?.let { rest ->
                        val open = rest.indexOf(" (")
                        day = if (open >= 0 && rest.endsWith(" chance)")) {
                            day.copy(precip = Quantity.parse(rest.substring(0, open)),
                                     chance = Quantity.parse(rest.substring(open + 2, rest.length - " chance)".length)))
                        } else day.copy(precip = Quantity.parse(rest))
                    }
                    ?: after("wind up to ", part)?.let { rest ->
                        val open = rest.indexOf(" (gusts ")
                        day = if (open >= 0 && rest.endsWith(")")) {
                            day.copy(wind = Quantity.parse(rest.substring(0, open)),
                                     gusts = Quantity.parse(rest.substring(open + " (gusts ".length, rest.length - 1)))
                        } else day.copy(wind = Quantity.parse(rest))
                    }
                    ?: after("UV ", part)?.let { day = day.copy(uv = Quantity.parse(it)) }
                    ?: after("sunrise ", part)?.let { if (isClock(it)) day = day.copy(sunrise = it) }
                    ?: after("sunset ", part)?.let { if (isClock(it)) day = day.copy(sunset = it) }
            }
            return day
        }
    }
}
