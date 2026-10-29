package dev.cedar.cedar.ui.cards

import android.content.Context
import android.text.format.DateFormat
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.OpenInNew
import androidx.compose.material.icons.outlined.Bedtime
import androidx.compose.material.icons.outlined.Eco
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.Flight
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.BaselineShift
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil.compose.AsyncImage
import dev.cedar.cedar.R
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cabinLabel
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.duration
import dev.cedar.cedar.ui.openUrl
import dev.cedar.cedar.ui.plural
import dev.cedar.cedar.ui.priceLevelLabel
import dev.cedar.cedar.ui.shortDateTime
import dev.cedar.core.FlightsCard
import dev.cedar.core.price
import dev.cedar.core.priceRange
import java.text.NumberFormat
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale
import kotlin.math.abs

// search_flights

/** A fare in the result's currency, without cents when it has none. */
private fun money(value: Double, currency: String): String {
    val digits = if (value == Math.floor(value)) 0 else 2
    return price(value, currency, digits, digits)
}

/** "Tue, Dec 1", as the user's locale writes a day. */
private fun day(date: LocalDate): String {
    val locale = Locale.getDefault()
    return DateTimeFormatter.ofPattern(DateFormat.getBestDateTimePattern(locale, "EEEdMMM"), locale).format(date)
}

/** A local time at an airport, as given (never converted to the phone's zone). */
private fun clock(time: LocalDateTime): String = DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT).format(time)

/** "2 adults, 1 child". */
private fun passengers(context: Context, p: FlightsCard.Passengers): String = buildList {
    add(context.resources.getQuantityString(R.plurals.flights_adults, p.adults, p.adults))
    if (p.children > 0) add(context.resources.getQuantityString(R.plurals.flights_children, p.children, p.children))
    if (p.infants > 0) add(context.resources.getQuantityString(R.plurals.flights_infants, p.infants, p.infants))
}.joinToString(", ")

/** "Nonstop", "1 stop (EWR)". */
private fun stops(context: Context, option: FlightsCard.Option): String {
    if (option.stops == 0) return context.getString(R.string.flights_nonstop)
    val text = context.resources.getQuantityString(R.plurals.flights_stops, option.stops, option.stops)
    val at = option.layovers.map { it.airport }
    return if (at.isEmpty()) text else "$text (${at.joinToString(", ")})"
}

/** "7 % less CO₂", in the user's locale. */
private fun co2(context: Context, pct: Int): String {
    val amount = NumberFormat.getPercentInstance().format(abs(pct) / 100.0)
    return when {
        pct < 0 -> context.getString(R.string.flights_co2_less, amount)
        pct > 0 -> context.getString(R.string.flights_co2_more, amount)
        else -> context.getString(R.string.flights_co2_typical)
    }
}

/**
 * A `search_flights` card. A search: the route, day, trip type, passengers and cabin; Google's price
 * insight with the price history; a row per flight (price, top pick, airline, times with "+1",
 * duration, stops and flags) that opens to its segments and layovers; how many more there are; and
 * the link to Google Flights with when the prices were read. A booking: the chosen flight, then who
 * sells it, for how much, with which fare and bags, and the Google Flights page to book on.
 */
@Composable
fun FlightsCardView(card: FlightsCard) {
    val context = LocalContext.current
    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        FlightsHeader(card)
        when (card.kind) {
            FlightsCard.Kind.FLIGHTS -> {
                card.insights?.let { PriceInsight(it, card.currency) }
                if (card.options.isEmpty()) {
                    Text(stringResource(R.string.flights_no_results), style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText)
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        card.options.forEach { FlightRow(it, card) }
                    }
                }
                if (card.more > 0) {
                    Text(plural(R.plurals.flights_more_options, card.more, card.more), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                }
            }
            FlightsCard.Kind.BOOKING -> {
                if (card.flights.isNotEmpty()) {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        CardLabel(stringResource(R.string.flights_chosen))
                        card.flights.forEach { FlightRow(it, card) }
                    }
                }
                if (card.sellers.isEmpty()) {
                    Text(stringResource(R.string.flights_no_booking), style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText)
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        CardLabel(stringResource(R.string.flights_where_to_book))
                        card.sellers.forEach { SellerRow(it, card.currency) }
                    }
                }
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            card.googleFlightsUrl?.let { url ->
                if (card.kind == FlightsCard.Kind.BOOKING && card.sellers.isNotEmpty()) {
                    Button(onClick = { openUrl(context, url) }) {
                        Icon(Icons.AutoMirrored.Outlined.OpenInNew, null, Modifier.size(16.dp))
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(R.string.flights_book_google))
                    }
                } else {
                    Row(Modifier.clickable { openUrl(context, url) }, verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.AutoMirrored.Outlined.OpenInNew, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
                        Spacer(Modifier.width(4.dp))
                        Text(stringResource(R.string.flights_open_google), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                    }
                }
            }
            card.asOf?.let {
                Text(stringResource(R.string.flights_as_of, shortDateTime(it)), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
            }
        }
    }
}

/** The route, with the airports' names when the flights give them, and what was searched for. */
@Composable
private fun FlightsHeader(card: FlightsCard) {
    val context = LocalContext.current
    Column {
        Text(card.route, style = MaterialTheme.typography.titleMedium)
        if (card.from.size == 1 && card.to.size == 1) {
            val all = card.options + card.flights
            val from = all.firstNotNullOfOrNull { o -> o.depart.name?.takeIf { o.depart.airport == card.from[0] } }
            val to = all.firstNotNullOfOrNull { o -> o.arrive.name?.takeIf { o.arrive.airport == card.to[0] } }
            if (from != null && to != null) {
                Text("$from → $to", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
        }
        val meta = buildList {
            card.date?.let { add(day(it)) }
            add(
                when (card.leg) {
                    FlightsCard.Leg.ONE_WAY -> context.getString(R.string.flights_one_way)
                    FlightsCard.Leg.OUTBOUND -> card.returnDate?.let { context.getString(R.string.flights_round_trip_back, day(it)) }
                        ?: context.getString(R.string.flights_one_way)
                    FlightsCard.Leg.RETURN -> context.getString(R.string.flights_return_flight)
                },
            )
            add(passengers(context, card.passengers))
            card.cabin?.let { add(context.getString(cabinLabel(it))) }
        }
        Text(meta.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
    }
}

/** "Prices are low for this route — typical 1,100–1,600 CHF", and the price history. */
@Composable
private fun PriceInsight(insights: FlightsCard.Insights, currency: String) {
    val color = when (insights.level) {
        "low" -> CedarColors.green
        "high" -> CedarColors.orange
        else -> MaterialTheme.colorScheme.primary
    }
    val level = insights.level?.let { l -> priceLevelLabel(l)?.let { stringResource(it) } ?: stringResource(R.string.flights_prices_level, l) }
        ?: stringResource(R.string.flights_prices_lowest, money(insights.lowest, currency))
    val low = insights.typicalLow
    val high = insights.typicalHigh
    val typical = if (low != null && high != null) stringResource(R.string.flights_typical_range, priceRange(low, high, currency, 0, 0)) else null
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(color.copy(alpha = 0.1f)).padding(10.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Text(listOfNotNull(level, typical).joinToString(" — "), style = MaterialTheme.typography.bodyMedium)
        if (insights.history.size >= 2) {
            Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Sparkline(insights.history.map { it.price }, color)
                Row {
                    Text(day(insights.history.first().date), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
                    Spacer(Modifier.weight(1f))
                    Text(stringResource(R.string.flights_price_history), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
                    Spacer(Modifier.weight(1f))
                    Text(day(insights.history.last().date), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
                }
            }
        }
    }
}

/** A small line of prices, oldest on the left. */
@Composable
private fun Sparkline(values: List<Double>, color: Color) {
    Canvas(Modifier.fillMaxWidth().height(36.dp)) {
        val lo = values.min()
        val hi = values.max()
        val span = if (hi > lo) hi - lo else 1.0
        val inset = 2.dp.toPx()
        val h = size.height - 2 * inset
        val step = size.width / (values.size - 1)
        val path = Path()
        values.forEachIndexed { i, v ->
            val x = i * step
            val y = inset + (h - ((v - lo) / span) * h).toFloat()
            if (i == 0) path.moveTo(x, y) else path.lineTo(x, y)
        }
        drawPath(path, color, style = Stroke(width = 2.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

/** An airline's logo on white (they are drawn for a light background), or a plane without one. */
@Composable
private fun AirlineLogo(url: String?, name: String?, size: Int) {
    Box(Modifier.size(size.dp).clip(RoundedCornerShape(6.dp)).background(Color.White), contentAlignment = Alignment.Center) {
        if (url != null) {
            AsyncImage(model = url, contentDescription = name, modifier = Modifier.size(size.dp).padding(2.dp), contentScale = ContentScale.Fit)
        } else {
            Icon(Icons.Outlined.Flight, name, Modifier.size((size * 0.6).dp), tint = Color.Gray)
        }
    }
}

@Composable
private fun TopPickBadge() {
    Text(
        stringResource(R.string.badge_top_pick), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold,
        color = MaterialTheme.colorScheme.onPrimary,
        modifier = Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.primary).padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

@Composable
private fun Flag(icon: ImageVector, text: String, color: Color) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, null, Modifier.size(12.dp), tint = color)
        Spacer(Modifier.width(3.dp))
        Text(text, style = MaterialTheme.typography.labelSmall, color = color)
    }
}

/** A flight: price, airline, times and stops; a tap opens its segments and layovers. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun FlightRow(option: FlightsCard.Option, card: FlightsCard) {
    val context = LocalContext.current
    var expanded by rememberSaveable(card, option.id) { mutableStateOf(false) }
    val depart = option.depart
    val arrive = option.arrive
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.surfaceContainerHighest.copy(alpha = 0.5f))
            .clickable { expanded = !expanded }.padding(10.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Row(verticalAlignment = Alignment.Top) {
            AirlineLogo(option.logo, option.airlines.firstOrNull(), 28)
            Spacer(Modifier.width(10.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                val secondary = CedarColors.secondaryText
                Text(
                    buildAnnotatedString {
                        if (depart.time != null && arrive.time != null) {
                            append("${clock(depart.time!!)} – ${clock(arrive.time!!)}")
                            if (option.daysLater > 0) {
                                withStyle(SpanStyle(fontSize = 10.sp, baselineShift = BaselineShift.Superscript, color = secondary)) {
                                    append("+${option.daysLater}")
                                }
                            }
                        } else {
                            append("${depart.airport} → ${arrive.airport}")
                        }
                    },
                    style = MaterialTheme.typography.titleSmall,
                )
                if (option.airlines.isNotEmpty()) {
                    Text(option.airlines.joinToString(", "), style = MaterialTheme.typography.bodySmall)
                }
                val facts = listOfNotNull(
                    "${depart.airport}–${arrive.airport}",
                    option.minutes?.let { context.duration(it) },
                    stops(context, option),
                )
                Text(facts.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = secondary)
            }
            Spacer(Modifier.width(8.dp))
            Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(2.dp)) {
                if (option.topPick) TopPickBadge()
                Text(option.price?.let { money(it, card.currency) } ?: "—", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
                if (option.price != null) {
                    val hint = listOfNotNull(
                        if (card.leg == FlightsCard.Leg.OUTBOUND) stringResource(R.string.flights_round_trip_price) else null,
                        if (card.passengers.total > 1) stringResource(R.string.flights_all_passengers) else null,
                    )
                    if (hint.isNotEmpty()) {
                        Text(
                            hint.joinToString(", "), Modifier.widthIn(max = 120.dp), style = MaterialTheme.typography.labelSmall,
                            color = CedarColors.secondaryText, textAlign = TextAlign.End,
                        )
                    }
                }
                Icon(
                    Icons.Outlined.ExpandMore, stringResource(if (expanded) R.string.collapse else R.string.expand),
                    Modifier.size(16.dp).rotate(if (expanded) 180f else 0f), tint = CedarColors.secondaryText,
                )
            }
        }
        val co2Pct = option.co2VsTypicalPct
        if (option.oftenDelayed || option.overnight || co2Pct != null) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                if (option.oftenDelayed) Flag(Icons.Outlined.Schedule, stringResource(R.string.flights_often_delayed), CedarColors.orange)
                if (option.overnight) Flag(Icons.Outlined.Bedtime, stringResource(R.string.flights_overnight), CedarColors.secondaryText)
                if (co2Pct != null) Flag(Icons.Outlined.Eco, co2(context, co2Pct), if (co2Pct < 0) CedarColors.green else CedarColors.secondaryText)
            }
        }
        if (expanded) FlightDetails(option)
    }
}

/** Each segment of a flight, with the layover after it. */
@Composable
private fun FlightDetails(option: FlightsCard.Option) {
    val context = LocalContext.current
    val secondary = CedarColors.secondaryText
    Column(Modifier.padding(start = 38.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        option.segments.forEachIndexed { i, s ->
            Row(verticalAlignment = Alignment.Top) {
                AirlineLogo(s.logo, s.airline, 20)
                Spacer(Modifier.width(8.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(1.dp)) {
                    listOf(s.from, s.to).forEach { stop ->
                        val place = stop.name?.let { "$it (${stop.airport})" } ?: stop.airport
                        Text(listOfNotNull(stop.time?.let(::clock), place).joinToString(" "), style = MaterialTheme.typography.bodySmall)
                    }
                    val facts = listOfNotNull(
                        s.flight, s.airline, s.aircraft, s.cabin,
                        s.legroom?.let { context.getString(R.string.flights_legroom, it) },
                        s.minutes?.let { context.duration(it) },
                    )
                    if (facts.isNotEmpty()) Text(facts.joinToString(" · "), style = MaterialTheme.typography.labelSmall, color = secondary)
                    if (s.oftenDelayed || s.overnight) {
                        Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            if (s.oftenDelayed) Flag(Icons.Outlined.Schedule, stringResource(R.string.flights_often_delayed), CedarColors.orange)
                            if (s.overnight) Flag(Icons.Outlined.Bedtime, stringResource(R.string.flights_overnight), secondary)
                        }
                    }
                }
            }
            option.layovers.getOrNull(i)?.takeIf { i < option.segments.size - 1 }?.let { l ->
                val place = l.name?.let { "$it (${l.airport})" } ?: l.airport
                val text = l.minutes?.let { context.getString(R.string.flights_layover, context.duration(it), place) } ?: place
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.Outlined.Schedule, null, Modifier.size(14.dp), tint = secondary)
                    Spacer(Modifier.width(6.dp))
                    Text(
                        if (l.overnight) "$text · ${stringResource(R.string.flights_overnight)}" else text,
                        style = MaterialTheme.typography.bodySmall, color = secondary,
                    )
                }
            }
        }
        option.co2Kg?.let {
            Text(
                stringResource(R.string.flights_co2_kg, NumberFormat.getIntegerInstance().format(it)),
                style = MaterialTheme.typography.labelSmall, color = secondary,
            )
        }
    }
}

/** Who sells a booking: the price, fare, bags, and whether it is a ticket of its own. */
@Composable
private fun SellerRow(seller: FlightsCard.Seller, currency: String) {
    val secondary = CedarColors.secondaryText
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
            .background(MaterialTheme.colorScheme.surfaceContainerHighest.copy(alpha = 0.5f)).padding(10.dp),
        verticalArrangement = Arrangement.spacedBy(3.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(seller.seller, Modifier.weight(1f), style = MaterialTheme.typography.titleSmall)
            Text(seller.price?.let { money(it, currency) } ?: "—", style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
        }
        seller.fare?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        when (seller.part) {
            FlightsCard.Part.TOGETHER -> {}
            FlightsCard.Part.DEPARTING -> Text(stringResource(R.string.flights_separate_departing), style = MaterialTheme.typography.labelSmall, color = CedarColors.orange)
            FlightsCard.Part.RETURNING -> Text(stringResource(R.string.flights_separate_returning), style = MaterialTheme.typography.labelSmall, color = CedarColors.orange)
        }
        seller.bags.forEach { Text("• $it", style = MaterialTheme.typography.bodySmall, color = secondary) }
        if (seller.marketedAs.isNotEmpty()) {
            Text(stringResource(R.string.flights_sold_as, seller.marketedAs.joinToString(", ")), style = MaterialTheme.typography.labelSmall, color = secondary)
        }
    }
}
