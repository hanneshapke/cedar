package dev.cedar.cedar.ui.cards

import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AcUnit
import androidx.compose.material.icons.filled.Bolt
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material.icons.filled.Dehaze
import androidx.compose.material.icons.filled.Grain
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.NightsStay
import androidx.compose.material.icons.filled.Navigation
import androidx.compose.material.icons.filled.WaterDrop
import androidx.compose.material.icons.filled.WbCloudy
import androidx.compose.material.icons.filled.WbSunny
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.cedar.cedar.R
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.openUrl
import dev.cedar.core.WeatherCard
import dev.cedar.core.number
import java.time.LocalDate
import java.time.format.DateTimeFormatter
import java.time.temporal.ChronoUnit
import java.util.Locale

// weather_forecast

/**
 * A weather card: the place and its local time, the current conditions with a large icon (a moon at
 * night), the temperature with feels like and the day's high and low, tiles for humidity, wind with its
 * direction, gusts, precipitation, UV and daylight, and a row per day with its icon, chance of rain and
 * a low-to-high temperature bar on a shared scale. A day row opens to show its details. The Open-Meteo
 * credit its CC BY 4.0 licence asks for closes the card.
 */
@Composable
fun WeatherCardView(card: WeatherCard) {
    val context = LocalContext.current
    val expanded = remember(card) { mutableStateListOf<String>() }
    val today = card.todayDay
    val now = card.now
    Column(
        Modifier.cardFrame().semantics { contentDescription = context.getString(R.string.weather_for, card.placeName) },
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        // Place
        Column {
            Text(card.placeName, style = MaterialTheme.typography.titleMedium)
            val sub = listOfNotNull(card.region, now?.time?.let { stringResource(R.string.weather_local_time, it) })
            if (sub.isNotEmpty()) Text(sub.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }

        // Now
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            WeatherIcon(card.leadCode, card.isNight, 54.dp)
            Column {
                now?.temp?.let { temp ->
                    Row(verticalAlignment = Alignment.Top) {
                        Text(WeatherFormat.whole(temp.value), fontSize = 46.sp, fontWeight = FontWeight.Light, lineHeight = 50.sp)
                        Text(temp.unit.ifEmpty { "°" }, Modifier.padding(top = 6.dp), style = MaterialTheme.typography.titleMedium, color = CedarColors.secondaryText)
                    }
                }
                card.leadCode?.let { Text(weatherCondition(it), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium) }
                val range = listOfNotNull(
                    now?.feels?.let { stringResource(R.string.weather_feels_like, WeatherFormat.degrees(it)) },
                    if (today?.high != null && today.low != null) stringResource(R.string.weather_high_low, WeatherFormat.degrees(today.high), WeatherFormat.degrees(today.low)) else null,
                )
                if (range.isNotEmpty()) Text(range.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
        }

        // Humidity, wind, gusts, precipitation, UV and daylight: whichever the report has.
        val tiles = buildList {
            now?.humidity?.let { add(StatItem(stringResource(R.string.weather_humidity), WeatherFormat.amount(it, 0))) }
            now?.wind?.let { wind ->
                val label = now.from?.let { stringResource(R.string.weather_wind_from, it) } ?: stringResource(R.string.weather_wind)
                add(StatItem(label, WeatherFormat.amount(wind), windFrom = now.from))
            }
            now?.gusts?.let { add(StatItem(stringResource(R.string.weather_gusts), WeatherFormat.amount(it))) }
            now?.precip?.let { add(StatItem(stringResource(R.string.weather_precipitation), WeatherFormat.amount(it, 2))) }
            today?.uv?.let { add(StatItem(stringResource(R.string.weather_uv), number(it.value, 0, 1))) }
            addAll(daylightTiles(today))
        }
        StatGrid(tiles)

        // Days
        val days = card.forecastDays
        if (days.isNotEmpty()) {
            val scale = remember(card) { TemperatureScale(days, now?.temp) }
            val anyChance = days.any { it.chance != null }
            Column {
                CardLabel(if (days.size == 1) stringResource(R.string.weather_forecast_one_day) else stringResource(R.string.weather_forecast_days, days.size))
                Spacer(Modifier.height(4.dp))
                days.forEachIndexed { index, day ->
                    val open = day.date in expanded
                    DayRow(day, index, card, scale, anyChance, open) { if (open) expanded.remove(day.date) else expanded.add(day.date) }
                    if (index < days.lastIndex) HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                }
            }
        }

        // Credit, as Open-Meteo's CC BY 4.0 licence asks
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(
                stringResource(R.string.weather_credit, "Open-Meteo.com"),
                Modifier.weight(1f).clickable { openUrl(context, "https://open-meteo.com/") },
                style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary,
            )
            card.timezone?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText) }
        }
    }
}

@Composable
private fun DayRow(
    day: WeatherCard.Day, index: Int, card: WeatherCard, scale: TemperatureScale, anyChance: Boolean, open: Boolean, toggle: () -> Unit,
) {
    val todayWord = stringResource(R.string.weather_today)
    val tomorrowWord = stringResource(R.string.weather_tomorrow)
    val names = remember(day.date, card.today, todayWord, tomorrowWord) {
        WeatherFormat.dayNames(day.date, index, card.today, todayWord, tomorrowWord)
    }
    Column(Modifier.fillMaxWidth().animateContentSize()) {
        Row(
            Modifier.fillMaxWidth().clickable(onClick = toggle).padding(vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Column(Modifier.width(68.dp)) {
                Text(names.name, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium, maxLines = 1)
                Text(names.short, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText, maxLines = 1)
            }
            Box(Modifier.width(26.dp), contentAlignment = Alignment.Center) { WeatherIcon(day.code, false, 22.dp) }
            if (anyChance) {
                val chance = day.chance?.takeIf { it.value >= 10 }?.let { WeatherFormat.amount(it, 0) } ?: ""
                Text(chance, Modifier.width(38.dp), style = MaterialTheme.typography.bodySmall, color = WeatherColors.rain, textAlign = TextAlign.End)
            }
            Text(WeatherFormat.degrees(day.low), Modifier.width(36.dp), style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText, textAlign = TextAlign.End)
            TemperatureBar(day, scale, if (day.date == card.today) card.now?.temp else null, Modifier.weight(1f))
            Text(WeatherFormat.degrees(day.high), Modifier.width(36.dp), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
            Icon(if (open) Icons.Filled.KeyboardArrowDown else Icons.Filled.KeyboardArrowRight, null, Modifier.size(18.dp), tint = CedarColors.secondaryText)
        }
        if (open) {
            val tiles = buildList {
                add(StatItem(stringResource(R.string.weather_conditions), day.code?.let { weatherCondition(it) } ?: "–"))
                day.precip?.let { precip ->
                    val amount = WeatherFormat.amount(precip, 2)
                    add(StatItem(stringResource(R.string.weather_precipitation), day.chance?.let { stringResource(R.string.weather_precip_chance, amount, WeatherFormat.amount(it, 0)) } ?: amount))
                }
                day.wind?.let { wind ->
                    add(StatItem(stringResource(R.string.weather_wind), day.gusts?.let { stringResource(R.string.weather_with_gusts, WeatherFormat.amount(wind), WeatherFormat.amount(it)) } ?: WeatherFormat.amount(wind)))
                }
                day.uv?.let { add(StatItem(stringResource(R.string.weather_uv), number(it.value, 0, 1))) }
                addAll(daylightTiles(day))
            }
            Box(Modifier.padding(bottom = 10.dp)) { StatGrid(tiles) }
        }
    }
}

/** A label over a value, for the current conditions and a day's details. */
private data class StatItem(val label: String, val value: String, val windFrom: String? = null)

/** The tiles two to a row. */
@Composable
private fun StatGrid(tiles: List<StatItem>) {
    if (tiles.isEmpty()) return
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        tiles.chunked(2).forEach { row ->
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                row.forEach { tile -> StatTile(tile, Modifier.weight(1f)) }
                if (row.size == 1) Spacer(Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun StatTile(tile: StatItem, modifier: Modifier) {
    Column(
        modifier.clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surfaceContainerHigh).padding(horizontal = 10.dp, vertical = 8.dp),
    ) {
        Text(tile.label, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText, maxLines = 1)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            val index = tile.windFrom?.let { WeatherCard.COMPASS.indexOf(it) } ?: -1
            // Points where the wind blows to: it comes from `windFrom`.
            if (index >= 0) Icon(Icons.Filled.Navigation, null, Modifier.size(14.dp).rotate(index * 22.5f + 180f))
            Text(tile.value, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium, maxLines = 2)
        }
    }
}

@Composable
private fun daylightTiles(day: WeatherCard.Day?): List<StatItem> {
    day ?: return emptyList()
    val rise = day.sunrise
    val set = day.sunset
    return when {
        rise != null && set != null -> listOf(StatItem(stringResource(R.string.weather_daylight), "$rise – $set"))
        rise != null -> listOf(StatItem(stringResource(R.string.weather_sunrise), rise))
        set != null -> listOf(StatItem(stringResource(R.string.weather_sunset), set))
        else -> emptyList()
    }
}

private object WeatherColors {
    val sun = Color(0xFFF2B01E)
    val moon = Color(0xFF7D8BD6)
    val rain = Color(0xFF3B82C4)
    val snow = Color(0xFF7FB6DF)
    val storm = Color(0xFFE08A1E)
}

/** The icon for a WMO code; clear and partly cloudy have a moon at night. */
@Composable
fun WeatherIcon(code: Int?, night: Boolean, size: Dp) {
    val (icon, tint) = weatherGlyph(WeatherCard.Sky.of(code), night)
    Icon(icon, code?.let { weatherCondition(it) }, Modifier.size(size), tint = tint)
}

@Composable
private fun weatherGlyph(sky: WeatherCard.Sky, night: Boolean): Pair<ImageVector, Color> = when (sky) {
    WeatherCard.Sky.CLEAR -> if (night) Icons.Filled.NightsStay to WeatherColors.moon else Icons.Filled.WbSunny to WeatherColors.sun
    WeatherCard.Sky.PARTLY -> if (night) Icons.Filled.NightsStay to WeatherColors.moon else Icons.Filled.WbCloudy to WeatherColors.sun
    WeatherCard.Sky.CLOUD -> Icons.Filled.Cloud to CedarColors.secondaryText
    WeatherCard.Sky.FOG -> Icons.Filled.Dehaze to CedarColors.secondaryText
    WeatherCard.Sky.DRIZZLE, WeatherCard.Sky.SLEET -> Icons.Filled.Grain to WeatherColors.rain
    WeatherCard.Sky.RAIN -> Icons.Filled.WaterDrop to WeatherColors.rain
    WeatherCard.Sky.SNOW -> Icons.Filled.AcUnit to WeatherColors.snow
    WeatherCard.Sky.STORM -> Icons.Filled.Bolt to WeatherColors.storm
}

/** The condition in the reader's language ("Partly cloudy"); unknown codes say so. */
@Composable
fun weatherCondition(code: Int): String {
    val id = when (code) {
        0 -> R.string.weather_code_0
        1 -> R.string.weather_code_1
        2 -> R.string.weather_code_2
        3 -> R.string.weather_code_3
        45 -> R.string.weather_code_45
        48 -> R.string.weather_code_48
        51 -> R.string.weather_code_51
        53 -> R.string.weather_code_53
        55 -> R.string.weather_code_55
        56 -> R.string.weather_code_56
        57 -> R.string.weather_code_57
        61 -> R.string.weather_code_61
        63 -> R.string.weather_code_63
        65 -> R.string.weather_code_65
        66 -> R.string.weather_code_66
        67 -> R.string.weather_code_67
        71 -> R.string.weather_code_71
        73 -> R.string.weather_code_73
        75 -> R.string.weather_code_75
        77 -> R.string.weather_code_77
        80 -> R.string.weather_code_80
        81 -> R.string.weather_code_81
        82 -> R.string.weather_code_82
        85 -> R.string.weather_code_85
        86 -> R.string.weather_code_86
        95 -> R.string.weather_code_95
        96 -> R.string.weather_code_96
        99 -> R.string.weather_code_99
        else -> return stringResource(R.string.weather_code_unknown, code)
    }
    return stringResource(id)
}

/**
 * Where the forecast's temperatures fall between the coldest low and the warmest high (the current
 * temperature included), as a fraction of the bar.
 */
class TemperatureScale(days: List<WeatherCard.Day>, now: WeatherCard.Quantity?) {
    val min: Double
    val span: Double

    init {
        val values = days.flatMap { listOfNotNull(it.low?.value, it.high?.value) } + listOfNotNull(now?.value)
        min = values.minOrNull() ?: 0.0
        span = maxOf((values.maxOrNull() ?: 1.0) - min, 1.0)
    }

    fun fraction(value: Double): Float = ((value - min) / span).coerceIn(0.0, 1.0).toFloat()
}

/** A day's low-to-high bar, coloured by temperature, with a dot for the current temperature today. */
@Composable
private fun TemperatureBar(day: WeatherCard.Day, scale: TemperatureScale, now: WeatherCard.Quantity?, modifier: Modifier) {
    val low = day.low ?: return
    val high = day.high ?: return
    val track = MaterialTheme.colorScheme.surfaceContainerHigh
    val dot = MaterialTheme.colorScheme.surface
    val ring = MaterialTheme.colorScheme.onSurface
    Canvas(modifier.height(10.dp)) {
        val radius = CornerRadius(size.height / 2)
        drawRoundRect(track, cornerRadius = radius)
        val start = scale.fraction(low.value) * size.width
        val end = maxOf(scale.fraction(high.value) * size.width, start + 6.dp.toPx())
        drawRoundRect(
            Brush.horizontalGradient(listOf(WeatherFormat.color(low.celsius), WeatherFormat.color(high.celsius)), startX = start, endX = end),
            topLeft = Offset(start, 0f), size = Size(end - start, size.height), cornerRadius = radius,
        )
        if (now != null) {
            val x = scale.fraction(now.value) * size.width
            drawCircle(dot, radius = 5.dp.toPx(), center = Offset(x, size.height / 2))
            drawCircle(ring, radius = 5.dp.toPx(), center = Offset(x, size.height / 2), style = androidx.compose.ui.graphics.drawscope.Stroke(1.5.dp.toPx()))
        }
    }
}

/** How the weather card writes its numbers and names, in the user's language. */
object WeatherFormat {
    data class DayNames(val name: String, val short: String, val long: String)

    fun whole(value: Double): String = number(Math.round(value).toDouble(), 0, 0)

    /** "10.2°C" → "10°"; the unit is shown once, by the current temperature. */
    fun degrees(q: WeatherCard.Quantity?): String = q?.let { "${whole(it.value)}°" } ?: "–"

    /** "4.6 km/h", "0.2 mm", "70%": the number in the reader's format with its unit. */
    fun amount(q: WeatherCard.Quantity, digits: Int = 1): String {
        val unit = if (q.unit == "inch") "in" else q.unit
        val n = number(q.value, 0, digits)
        if (unit.isEmpty()) return n
        return if (unit == "%" || unit.startsWith("°")) "$n$unit" else "$n $unit"
    }

    /** Cold blue through mild green and yellow to hot red, for a temperature in °C, as the web draws it. */
    fun color(celsius: Double): Color = Color.hsl(WeatherCard.hue(celsius).toFloat(), 0.75f, 0.55f)

    /** "Today", "Tomorrow", else the weekday; and the date, short and long, in the reader's format. */
    fun dayNames(date: String, index: Int, today: String?, todayWord: String, tomorrowWord: String, locale: Locale = Locale.getDefault()): DayNames {
        val day = runCatching { LocalDate.parse(date) }.getOrNull() ?: return DayNames(date, "", date)
        var name = DateTimeFormatter.ofPattern("EEE", locale).format(day)
        val todayDate = today?.let { runCatching { LocalDate.parse(it) }.getOrNull() }
        if (todayDate != null) {
            when (ChronoUnit.DAYS.between(todayDate, day)) {
                0L -> name = todayWord
                1L -> name = tomorrowWord
            }
        } else if (index == 0) {
            name = todayWord
        }
        return DayNames(name, DateTimeFormatter.ofPattern("d MMM", locale).format(day), DateTimeFormatter.ofPattern("EEEE d MMMM", locale).format(day))
    }
}
