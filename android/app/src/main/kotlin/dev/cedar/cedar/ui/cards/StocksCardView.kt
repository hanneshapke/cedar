package dev.cedar.cedar.ui.cards

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.openUrl
import dev.cedar.cedar.ui.relative
import dev.cedar.cedar.ui.shortDateTime
import dev.cedar.core.StockFormat
import dev.cedar.core.StocksCard
import java.time.Instant
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

private fun changeColor(x: Double?): Color = when {
    x == null || abs(x) < 0.005 -> Color.Unspecified
    x > 0 -> CedarColors.green
    else -> CedarColors.red
}

private fun callColor(call: StocksCard.Call): Color = when (call) {
    StocksCard.Call.BUY -> CedarColors.green
    StocksCard.Call.HOLD -> CedarColors.orange
    StocksCard.Call.SELL -> CedarColors.red
}

/** A line on the chart: a symbol, or the benchmark when it isn't one of them. */
private data class StockLine(val key: String, val label: String, val series: StocksCard.Series, val isBenchmark: Boolean)

private fun stockLines(card: StocksCard): List<StockLine> =
    card.stocks.map { StockLine(it.symbol, it.symbol, it.series, false) } +
        listOfNotNull(card.benchmark?.let { b -> b.series?.let { StockLine(b.symbol, b.name, it, true) } })

/** The periods the data covers, with some slack for weekends and holidays. */
private fun offeredRanges(card: StocksCard, lines: List<StockLine>): List<String> {
    val first = lines.mapNotNull { it.series.dates.firstOrNull() }.minOrNull() ?: return listOf(card.range)
    val last = lines.mapNotNull { it.series.dates.lastOrNull() }.maxOrNull() ?: return listOf(card.range)
    return StocksCard.RANGES.filter { (key, days) -> key == card.range || !last.minusSeconds(days * 86400L).isBefore(first.minusSeconds(7 * 86400L)) }.map { it.first }
}

private val dayFormat = DateTimeFormatter.ofPattern("d MMM yyyy").withZone(ZoneOffset.UTC)

/**
 * A `compare_stocks` card: a quote tile per symbol (which also picks the symbol shown below), a chart
 * of % change against the benchmark or one symbol's price, the selected symbol's rating, the returns
 * side by side, and the news.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun StocksCardView(card: StocksCard) {
    val context = LocalContext.current
    val lines = remember(card) { stockLines(card) }
    val offered = remember(card) { offeredRanges(card, lines) }
    var selected by remember(card) { mutableStateOf(card.stocks[0].symbol) }
    var range by remember(card) { mutableStateOf(if (card.range in offered) card.range else offered.lastOrNull() ?: "1Y") }
    var percent by remember(card) { mutableStateOf(lines.size > 1) }
    var allNews by remember(card) { mutableStateOf(false) }
    val stock = card.stocks.firstOrNull { it.symbol == selected } ?: card.stocks[0]

    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        // Quote tiles.
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            card.stocks.forEach { s ->
                val isSelected = s.symbol == selected
                val accent = MaterialTheme.colorScheme.primary
                Column(
                    Modifier
                        .widthIn(min = 108.dp)
                        .clip(RoundedCornerShape(10.dp))
                        .background(if (isSelected) accent.copy(alpha = 0.1f) else MaterialTheme.colorScheme.surfaceContainerHighest.copy(alpha = 0.5f))
                        .border(1.dp, if (isSelected) accent.copy(alpha = 0.6f) else Color.Transparent, RoundedCornerShape(10.dp))
                        .clickable { selected = s.symbol }
                        .padding(10.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(s.symbol, style = MaterialTheme.typography.titleSmall, fontWeight = FontWeight.Bold)
                        s.rating?.let { CallPill(it.call, small = true) }
                    }
                    Text(StockFormat.money(s.price, s.currency), style = MaterialTheme.typography.bodyMedium)
                    Text(StockFormat.signed(s.changePct, 2), style = MaterialTheme.typography.labelMedium, color = changeColor(s.changePct))
                }
            }
        }

        // The chart.
        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                SingleChoiceSegmentedButtonRow(Modifier.weight(1f)) {
                    offered.forEachIndexed { i, key ->
                        SegmentedButton(selected = range == key, onClick = { range = key }, shape = SegmentedButtonDefaults.itemShape(i, offered.size), icon = {}) {
                            Text(key, style = MaterialTheme.typography.labelSmall, maxLines = 1)
                        }
                    }
                }
                if (lines.size > 1) {
                    SingleChoiceSegmentedButtonRow(Modifier.width(96.dp)) {
                        SegmentedButton(selected = percent, onClick = { percent = true }, shape = SegmentedButtonDefaults.itemShape(0, 2), icon = {}) { Text("%") }
                        SegmentedButton(selected = !percent, onClick = { percent = false }, shape = SegmentedButtonDefaults.itemShape(1, 2), icon = {}) {
                            Text(StockFormat.currencySymbol(stock.currency).take(3), maxLines = 1)
                        }
                    }
                }
            }
            val newest = lines.mapNotNull { it.series.dates.lastOrNull() }.maxOrNull()
            if (newest != null) {
                val days = StocksCard.RANGES.firstOrNull { it.first == range }?.second ?: 365
                val from = newest.minusSeconds(days * 86400L)
                val shown = if (percent) lines else lines.filter { it.key == selected }
                // Each shown series from its first point inside the period; % change counts from there.
                val chartLines = shown.mapIndexed { index, line ->
                    val s = line.series
                    val start = s.dates.indexOfFirst { !it.isBefore(from) }.takeIf { it >= 0 } ?: 0
                    val base = s.closes[start]
                    val stride = max(1, (s.dates.size - start) / 260)
                    val indices = (start until s.dates.size step stride).toMutableList().apply { if (this.last() != s.dates.size - 1) add(s.dates.size - 1) }
                    Line(
                        label = line.label,
                        color = if (line.isBenchmark) Color.Gray else seriesColor(index),
                        points = indices.map { i -> s.dates[i].epochSecond.toDouble() to if (percent) (s.closes[i] / base - 1) * 100 else s.closes[i] },
                        dashed = line.isBenchmark,
                        fill = !percent,
                    )
                }
                LineChart(
                    chartLines,
                    yFormat = { v -> if (percent) StockFormat.signed(v) else StockFormat.money(v, stock.currency) },
                    xFormat = { x -> dayFormat.format(Instant.ofEpochSecond(x.toLong())) },
                    zeroRule = percent,
                    includeZero = percent,
                )
                if (chartLines.size > 1) Legend(chartLines.map { it.label to it.color }, emptyList()) {}
            }
        }

        RatingPanel(stock, card.benchmark?.name)

        // Returns.
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            CardLabel("Returns")
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text("", style = MaterialTheme.typography.labelSmall)
                    card.stocks.forEach { Text(it.symbol, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.height(20.dp)) }
                }
                StocksCard.RETURN_PERIODS.forEachIndexed { p, (_, label) ->
                    Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(4.dp)) {
                        Text(label, style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold, color = CedarColors.secondaryText)
                        card.stocks.forEach { s ->
                            val value = s.returns[p]
                            val color = changeColor(value)
                            Text(
                                StockFormat.signed(value),
                                style = MaterialTheme.typography.labelMedium,
                                color = color,
                                modifier = Modifier.height(20.dp).clip(RoundedCornerShape(4.dp))
                                    .background(if (color == Color.Unspecified) Color.Transparent else color.copy(alpha = (min(1.0, abs(value ?: 0.0) / 40) * 0.25).toFloat()))
                                    .padding(horizontal = 5.dp, vertical = 1.dp),
                            )
                        }
                    }
                }
            }
        }

        // News.
        if (card.news.isNotEmpty() || card.newsNote != null) {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                CardLabel("News")
                val shown = if (allNews) card.news else card.news.take(4)
                shown.forEach { item ->
                    Column(Modifier.fillMaxWidth().clickable { openUrl(context, item.url) }) {
                        Text(item.title, style = MaterialTheme.typography.bodyMedium)
                        val meta = item.symbols + listOfNotNull(item.publisher, item.published?.let(::relative))
                        Text(meta.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                    }
                }
                if (card.news.size > shown.size) {
                    Text("Show all ${card.news.size} headlines", style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary,
                        modifier = Modifier.clickable { allNews = true })
                }
                card.newsNote?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
            }
        }

        // Footer.
        Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
            val small = MaterialTheme.typography.labelSmall
            card.unavailable.forEach { Text("${it.symbol}: ${it.reason ?: "not available"}", style = small, color = CedarColors.secondaryText) }
            card.asOf?.let { Text("As of ${shortDateTime(it)}", style = small, color = CedarColors.secondaryText) }
            card.disclaimer?.let { Text(it, style = small, color = CedarColors.secondaryText.copy(alpha = 0.7f)) }
        }
    }
}

/** Buy, hold, or sell, in its colour. */
@Composable
private fun CallPill(call: StocksCard.Call, small: Boolean) {
    val color = callColor(call)
    Text(
        call.label,
        style = if (small) MaterialTheme.typography.labelSmall else MaterialTheme.typography.labelLarge,
        fontWeight = FontWeight.Bold,
        color = color,
        modifier = Modifier.clip(RoundedCornerShape(50)).background(color.copy(alpha = 0.15f))
            .padding(horizontal = if (small) 5.dp else 10.dp, vertical = if (small) 1.dp else 4.dp),
    )
}

/**
 * The selected symbol's rating: the call on a sell–hold–buy gauge, the summary, every factor with its
 * score and reasoning, what would change the view, and the key numbers.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun RatingPanel(stock: StocksCard.Stock, benchmark: String?) {
    Column(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.surface.copy(alpha = 0.6f)).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val r = stock.rating
        if (r != null) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                CallPill(r.call, small = false)
                Column {
                    Text("${stock.symbol} ${stock.name}", style = MaterialTheme.typography.titleSmall, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        "${r.confidence.replaceFirstChar { it.uppercase() }} confidence${r.lean?.let { " · $it" } ?: ""}",
                        style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText,
                    )
                }
            }
            ScoreGauge(r)
            r.summary?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
            if (r.factors.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                    CardLabel("Why: the factors behind the call")
                    r.factors.forEach { f ->
                        Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(f.name, style = MaterialTheme.typography.titleSmall)
                                f.headline?.let {
                                    Spacer(Modifier.width(6.dp))
                                    Text(it, Modifier.weight(1f, fill = false), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                }
                                Spacer(Modifier.weight(1f))
                                ScoreBar(f.score)
                            }
                            Text(f.detail, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                        }
                    }
                }
            }
            if (r.watch.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    CardLabel("What would change the view")
                    r.watch.forEach { w ->
                        Row {
                            Icon(Icons.Outlined.Visibility, null, Modifier.size(14.dp), tint = CedarColors.secondaryText)
                            Spacer(Modifier.width(6.dp))
                            Text(w, style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        } else {
            Text("${stock.symbol} ${stock.name}: no rating", style = MaterialTheme.typography.titleSmall)
        }
        // The key numbers.
        val m = { v: Double -> StockFormat.money(v, stock.currency) }
        val i = stock.indicators
        val items = buildList {
            if (stock.low52 != null && stock.high52 != null) add("52-week range" to "${m(stock.low52!!)} – ${m(stock.high52!!)}")
            if (stock.dayLow != null && stock.dayHigh != null) add("Day range" to "${m(stock.dayLow!!)} – ${m(stock.dayHigh!!)}")
            i.sma50?.let { add("50-day avg" to m(it)) }
            i.sma200?.let { add("200-day avg" to m(it)) }
            i.rsi?.let { add("RSI (14)" to "%.0f".format(it)) }
            i.volatility?.let { add("Volatility" to "%.0f%% / yr".format(it)) }
            i.drawdown?.let { add("Max drawdown (1Y)" to StockFormat.signed(it)) }
            i.beta?.let { add("Beta vs ${benchmark ?: "index"}" to "%.2f".format(it)) }
        }
        FlowRow(horizontalArrangement = Arrangement.spacedBy(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items.forEach { (label, value) ->
                Column(Modifier.widthIn(min = 130.dp)) {
                    Text(label, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
                    Text(value, style = MaterialTheme.typography.labelLarge)
                }
            }
        }
    }
}

/** The score on a scale from −max (sell) to +max (buy). */
@Composable
private fun ScoreGauge(rating: StocksCard.Rating) {
    val fraction = ((rating.score + rating.maxScore).toFloat() / (2 * rating.maxScore)).coerceIn(0f, 1f)
    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Box(Modifier.fillMaxWidth().height(8.dp).clip(RoundedCornerShape(4.dp))) {
            Row(Modifier.fillMaxWidth().height(8.dp)) {
                Box(Modifier.weight(1f).height(8.dp).background(CedarColors.red.copy(alpha = 0.5f)))
                Box(Modifier.weight(1f).height(8.dp).background(CedarColors.orange.copy(alpha = 0.5f)))
                Box(Modifier.weight(1f).height(8.dp).background(CedarColors.green.copy(alpha = 0.5f)))
            }
            Row(Modifier.fillMaxWidth()) {
                if (fraction > 0f) Spacer(Modifier.weight(fraction))
                Box(Modifier.width(4.dp).height(8.dp).background(MaterialTheme.colorScheme.onSurface))
                if (fraction < 1f) Spacer(Modifier.weight(1f - fraction))
            }
        }
        Row(Modifier.fillMaxWidth()) {
            val small = MaterialTheme.typography.labelSmall
            Text("Sell", style = small, color = CedarColors.secondaryText)
            Spacer(Modifier.weight(1f))
            Text("Hold", style = small, color = CedarColors.secondaryText)
            Spacer(Modifier.weight(1f))
            Text("Buy", style = small, color = CedarColors.secondaryText)
        }
    }
}

/** A factor's score, −2…2, as five segments. */
@Composable
private fun ScoreBar(score: Int) {
    Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
        (-2..2).forEach { step ->
            val on = (score > 0 && step in 1..score) || (score < 0 && step in score..-1) || (score == 0 && step == 0)
            val color = when {
                !on -> MaterialTheme.colorScheme.outlineVariant
                score > 0 -> CedarColors.green
                score < 0 -> CedarColors.red
                else -> CedarColors.orange
            }
            Box(Modifier.size(width = 8.dp, height = 6.dp).clip(RoundedCornerShape(2.dp)).background(color))
        }
    }
}
