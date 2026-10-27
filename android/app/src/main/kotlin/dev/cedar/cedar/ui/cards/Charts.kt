package dev.cedar.cedar.ui.cards

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.cedar.cedar.R
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.chartKindLabel
import dev.cedar.core.ChartCard
import java.text.NumberFormat
import kotlin.math.abs
import kotlin.math.atan2
import kotlin.math.cos
import kotlin.math.floor
import kotlin.math.log10
import kotlin.math.max
import kotlin.math.min
import kotlin.math.pow
import kotlin.math.sin
import kotlin.math.sqrt

/** The categorical palette (as the web UI's chart-view.js); series past it are grey. */
val ChartPalette = listOf(
    Color(0xFF3A7D5C), Color(0xFFD9822B), Color(0xFF4C78A8), Color(0xFFB4566F),
    Color(0xFF7A6FB0), Color(0xFF5BA3A0), Color(0xFFC9A227), Color(0xFF8C6D54),
)

fun seriesColor(index: Int): Color = ChartPalette.getOrElse(index) { Color(0xFF9E9E9E) }

/** One line on a [LineChart]: points as (x, y), x increasing. */
data class Line(val label: String, val color: Color, val points: List<Pair<Double, Double?>>, val dashed: Boolean = false, val fill: Boolean = false)

/** Rounded steps for an axis from `lo` to `hi`. */
fun niceTicks(lo: Double, hi: Double, count: Int = 4): List<Double> {
    if (!lo.isFinite() || !hi.isFinite() || hi <= lo) return listOf(lo)
    val raw = (hi - lo) / count
    val magnitude = 10.0.pow(floor(log10(raw)))
    val step = listOf(1.0, 2.0, 2.5, 5.0, 10.0).map { it * magnitude }.first { it >= raw }
    val start = kotlin.math.ceil(lo / step) * step
    return generateSequence(start) { it + step }.takeWhile { it <= hi + step * 1e-9 }.toList()
}

private fun DrawScope.label(measurer: TextMeasurer, text: String, at: Offset, color: Color, alignEnd: Boolean = false, center: Boolean = false) {
    val layout = measurer.measure(text, TextStyle(fontSize = 10.sp, color = color))
    val x = when {
        alignEnd -> at.x - layout.size.width
        center -> at.x - layout.size.width / 2
        else -> at.x
    }
    drawText(layout, topLeft = Offset(x, at.y - layout.size.height / 2))
}

/**
 * Lines (or areas) over a numeric x axis, with a value axis, an optional shaded band, and a probe:
 * touch or drag to read every line's value at that x.
 */
@Composable
fun LineChart(
    lines: List<Line>,
    yFormat: (Double) -> String,
    xFormat: (Double) -> String,
    modifier: Modifier = Modifier,
    includeZero: Boolean = false,
    zeroRule: Boolean = false,
    stacked: Boolean = false,
    log: Boolean = false,
    band: ChartCard.Band? = null,
    scatter: Boolean = false,
) {
    val measurer = rememberTextMeasurer()
    var probe by remember(lines) { mutableStateOf<Float?>(null) }
    val grid = MaterialTheme.colorScheme.outlineVariant
    val ink = CedarColors.secondaryText
    val surface = MaterialTheme.colorScheme.surface
    val onSurface = MaterialTheme.colorScheme.onSurface

    // Stacked areas add each line to the ones before it.
    val shown = if (!stacked) lines else {
        val running = mutableMapOf<Double, Double>()
        lines.map { line -> line.copy(points = line.points.map { (x, y) -> val sum = (running[x] ?: 0.0) + (y ?: 0.0); running[x] = sum; x to sum }) }
    }
    val xs = shown.flatMap { l -> l.points.map { it.first } }
    val ys = shown.flatMap { l -> l.points.mapNotNull { it.second } }.filter { !log || it > 0 } + listOfNotNull(band?.from, band?.to)
    if (xs.isEmpty() || ys.isEmpty()) return
    val xMin = xs.min()
    val xMax = xs.max().let { if (it == xMin) it + 1 else it }
    var yMin = ys.min()
    var yMax = ys.max()
    if (includeZero || stacked) {
        yMin = min(yMin, 0.0)
        yMax = max(yMax, 0.0)
    }
    if (yMin == yMax) {
        yMin -= 1
        yMax += 1
    }
    val pad = (yMax - yMin) * 0.06
    val lo = if (log) yMin else yMin - if (yMin == 0.0) 0.0 else pad
    val hi = yMax + pad
    fun ty(v: Double): Double = if (log) log10(max(v, 1e-12)) else v
    val tLo = ty(lo)
    val tHi = ty(hi)
    val ticks = if (log) generateSequence(10.0.pow(floor(log10(lo)))) { it * 10 }.takeWhile { it <= hi }.filter { it >= lo }.toList().ifEmpty { listOf(lo, hi) } else niceTicks(lo, hi)

    Canvas(
        modifier
            .fillMaxWidth()
            .height(220.dp)
            .pointerInput(lines) {
                detectTapGestures { probe = if (probe == null) it.x else null }
            }
            .pointerInput(lines) {
                detectDragGestures(onDragEnd = {}) { change, _ -> probe = change.position.x }
            },
    ) {
        val left = 44.dp.toPx()
        val bottom = size.height - 18.dp.toPx()
        val top = 8.dp.toPx()
        val right = size.width - 6.dp.toPx()
        fun px(x: Double) = (left + (x - xMin) / (xMax - xMin) * (right - left)).toFloat()
        fun py(y: Double) = (bottom - (ty(y) - tLo) / (tHi - tLo) * (bottom - top)).toFloat()

        band?.let {
            drawRect(CedarColors.green.copy(alpha = 0.12f), Offset(left, py(it.to)), Size(right - left, py(it.from) - py(it.to)))
            it.label?.let { text -> label(measurer, text, Offset(right - 4.dp.toPx(), py(it.to) + 8.dp.toPx()), ink, alignEnd = true) }
        }
        for (t in ticks) {
            val y = py(t)
            drawLine(grid.copy(alpha = 0.5f), Offset(left, y), Offset(right, y), 1f)
            label(measurer, yFormat(t), Offset(left - 4.dp.toPx(), y), ink, alignEnd = true)
        }
        if (zeroRule && lo < 0 && hi > 0) drawLine(grid, Offset(left, py(0.0)), Offset(right, py(0.0)), 1.5f)
        label(measurer, xFormat(xMin), Offset(left, size.height - 8.dp.toPx()), ink)
        label(measurer, xFormat(xs.max()), Offset(right, size.height - 8.dp.toPx()), ink, alignEnd = true)

        shown.asReversed().forEach { line ->
            val pts = line.points.mapNotNull { (x, y) -> y?.takeIf { !log || it > 0 }?.let { Offset(px(x), py(it)) } }
            if (scatter) {
                pts.forEach { drawCircle(line.color, 3.dp.toPx(), it) }
                return@forEach
            }
            // A gap (null) breaks the line.
            val path = Path()
            var pen = false
            line.points.forEach { (x, y) ->
                if (y == null || (log && y <= 0)) {
                    pen = false
                } else {
                    val p = Offset(px(x), py(y))
                    if (pen) path.lineTo(p.x, p.y) else path.moveTo(p.x, p.y)
                    pen = true
                }
            }
            if (line.fill && pts.size > 1) {
                val area = Path().apply {
                    moveTo(pts.first().x, bottom)
                    pts.forEach { lineTo(it.x, it.y) }
                    lineTo(pts.last().x, bottom)
                    close()
                }
                drawPath(area, line.color.copy(alpha = 0.22f))
            }
            drawPath(
                path, line.color,
                style = Stroke(
                    width = (if (line.dashed) 1.5 else 2.2).dp.toPx(),
                    pathEffect = if (line.dashed) PathEffect.dashPathEffect(floatArrayOf(10f, 7f)) else null,
                ),
            )
            if (pts.size == 1) drawCircle(line.color, 3.dp.toPx(), pts[0])
        }

        probe?.let { at ->
            val x = xMin + ((at - left) / (right - left)).coerceIn(0f, 1f) * (xMax - xMin)
            // The nearest x any line has.
            val nearest = xs.minByOrNull { abs(it - x) } ?: return@let
            val cx = px(nearest)
            drawLine(ink.copy(alpha = 0.6f), Offset(cx, top), Offset(cx, bottom), 1f)
            val rows = listOf(xFormat(nearest)) + lines.mapNotNull { line ->
                line.points.firstOrNull { it.first == nearest }?.second?.let { "${line.label} ${yFormat(it)}" }
            }
            val layouts = rows.mapIndexed { i, r -> measurer.measure(r, TextStyle(fontSize = 10.sp, color = onSurface, fontWeight = if (i == 0) FontWeight.SemiBold else FontWeight.Normal)) }
            val w = layouts.maxOf { it.size.width } + 12.dp.toPx()
            val h = layouts.sumOf { it.size.height } + 10.dp.toPx()
            val bx = (cx + 8.dp.toPx()).let { if (it + w > right) cx - 8.dp.toPx() - w else it }.coerceAtLeast(left)
            drawRoundRect(surface.copy(alpha = 0.95f), Offset(bx, top), Size(w, h), CornerRadius(6.dp.toPx()))
            drawRoundRect(grid, Offset(bx, top), Size(w, h), CornerRadius(6.dp.toPx()), style = Stroke(1f))
            var y = top + 5.dp.toPx()
            layouts.forEach {
                drawText(it, topLeft = Offset(bx + 6.dp.toPx(), y))
                y += it.size.height
            }
        }
    }
}

/** A legend whose entries hide and show their series. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun Legend(entries: List<Pair<String, Color>>, hidden: List<String>, toggle: (String) -> Unit) {
    FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        entries.forEach { (name, color) ->
            Row(
                Modifier.clip(RoundedCornerShape(6.dp)).clickable { toggle(name) }.padding(2.dp).alpha(if (name in hidden) 0.4f else 1f),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(10.dp).clip(CircleShape).background(color))
                Spacer(Modifier.width(4.dp))
                Text(name, style = MaterialTheme.typography.labelSmall)
            }
        }
    }
}

// generate_chart

/** A chart drawn from a `generate_chart` call's data (or a `[chart]` line a tool printed): legend toggles, touch to read values, the data as a table. */
@Composable
fun ChartCardView(card: ChartCard) {
    val hidden = remember(card) { mutableStateListOf<String>() }
    var showTable by remember(card) { mutableStateOf(false) }
    val format: (Double) -> String = { ChartCard.format(it) }
    val names = seriesNames(card)
    val visible = card.series.withIndex().filter { names[it.index] !in hidden }
    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Column {
            Text(card.title ?: stringResource(chartKindLabel(card.kind)), style = MaterialTheme.typography.titleMedium)
            listOfNotNull(card.yLabel, card.xLabel?.let { stringResource(R.string.chart_by, it) }).takeIf { it.isNotEmpty() }?.let {
                Text(it.joinToString(" "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
        }
        when (card.kind) {
            ChartCard.Kind.PIE -> PieChart(card)
            ChartCard.Kind.BAR, ChartCard.Kind.BARH, ChartCard.Kind.HISTOGRAM -> BarChart(card, names, hidden)
            else -> {
                val numeric = card.numericX
                val lines = visible.map { (i, s) ->
                    Line(names[i], seriesColor(i), s.values.mapIndexed { j, v -> (if (numeric) card.labels[j].number!! else j.toDouble()) to v },
                        fill = card.kind == ChartCard.Kind.AREA)
                }
                LineChart(
                    lines, format,
                    xFormat = { x -> if (numeric) ChartCard.format(x) else card.labels.getOrNull(x.toInt())?.text ?: "" },
                    includeZero = card.kind == ChartCard.Kind.AREA,
                    stacked = card.stacked,
                    log = card.log,
                    band = card.band,
                    scatter = card.kind == ChartCard.Kind.SCATTER,
                )
            }
        }
        if (card.series.size > 1 && card.kind != ChartCard.Kind.PIE) {
            Legend(names.mapIndexed { i, name -> name to seriesColor(i) }, hidden) { name -> if (name in hidden) hidden.remove(name) else hidden.add(name) }
        }
        if (card.thinned) {
            Text(stringResource(R.string.chart_thinned), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        }
        Row(Modifier.clickable { showTable = !showTable }, verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Outlined.ChevronRight, null, Modifier.size(16.dp).rotate(if (showTable) 90f else 0f), tint = CedarColors.secondaryText)
            Text(stringResource(R.string.chart_data), style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText)
        }
        if (showTable) DataTable(card, names)
    }
}

/** Each series' name: the model's, or "Series 2" in the user's language for one it didn't name. */
@Composable
private fun seriesNames(card: ChartCard): List<String> =
    card.series.mapIndexed { i, s -> if (s.named) s.name else stringResource(R.string.chart_series, i + 1) }

/** Bars, horizontal bars or a histogram's bins; stacked when the call asked. */
@Composable
private fun BarChart(card: ChartCard, names: List<String>, hidden: List<String>) {
    val measurer = rememberTextMeasurer()
    val ink = CedarColors.secondaryText
    val grid = MaterialTheme.colorScheme.outlineVariant
    val histogram = card.kind == ChartCard.Kind.HISTOGRAM
    val hist = if (histogram) card.histogram else null
    val categories: List<String> = hist?.first?.windowed(2)?.map { (a, b) -> "${ChartCard.format(a)}–${ChartCard.format(b)}" } ?: card.labels.map { it.text }
    val series = card.series.withIndex().filter { names[it.index] !in hidden }.map { (i, s) ->
        Triple(names[i], seriesColor(i), if (hist != null) hist.second[i].map { it.toDouble() } else s.values.map { it ?: 0.0 })
    }
    if (series.isEmpty() || categories.isEmpty()) return
    val horizontal = card.kind == ChartCard.Kind.BARH
    val stacked = card.stacked || (histogram && series.size == 1)
    val totals = categories.indices.map { c -> if (stacked) series.sumOf { it.third.getOrElse(c) { 0.0 } } else series.maxOf { it.third.getOrElse(c) { 0.0 } } }
    val lowest = if (stacked) 0.0 else min(0.0, series.minOf { s -> s.third.minOrNull() ?: 0.0 })
    val highest = max(totals.maxOrNull() ?: 1.0, 0.0).let { if (it == lowest) it + 1 else it }
    val ticks = niceTicks(lowest, highest)
    var probe by remember(card) { mutableStateOf<Int?>(null) }
    val height = if (horizontal) (categories.size * 22 + 24).coerceIn(120, 600) else 220
    Canvas(
        Modifier.fillMaxWidth().height(height.dp).pointerInput(card) {
            detectTapGestures { pos ->
                val axis = if (horizontal) pos.y / size.height else (pos.x - 44.dp.toPx()) / (size.width - 50.dp.toPx())
                val i = (axis * categories.size).toInt().coerceIn(0, categories.size - 1)
                probe = if (probe == i) null else i
            }
        },
    ) {
        val left = if (horizontal) 80.dp.toPx() else 44.dp.toPx()
        val right = size.width - 6.dp.toPx()
        val top = 6.dp.toPx()
        val bottom = size.height - 18.dp.toPx()
        val span = highest - lowest
        fun v(x: Double) = ((x - lowest) / span).toFloat()
        val n = categories.size
        for (t in ticks) {
            if (horizontal) {
                val x = left + v(t) * (right - left)
                drawLine(grid.copy(alpha = 0.5f), Offset(x, top), Offset(x, bottom), 1f)
                label(measurer, ChartCard.format(t), Offset(x, size.height - 8.dp.toPx()), ink, center = true)
            } else {
                val y = bottom - v(t) * (bottom - top)
                drawLine(grid.copy(alpha = 0.5f), Offset(left, y), Offset(right, y), 1f)
                label(measurer, ChartCard.format(t), Offset(left - 4.dp.toPx(), y), ink, alignEnd = true)
            }
        }
        val slot = (if (horizontal) bottom - top else right - left) / n
        val gap = if (histogram) 1f else slot * 0.18f
        categories.indices.forEach { c ->
            var base = 0.0
            series.forEachIndexed { k, (_, color, values) ->
                val value = values.getOrElse(c) { 0.0 }
                val (from, to) = if (stacked) base to base + value else 0.0 to value
                base += if (stacked) value else 0.0
                val width = if (stacked) slot - 2 * gap else (slot - 2 * gap) / series.size
                val offset = c * slot + gap + if (stacked) 0f else k * width
                val a = v(min(from, to))
                val b = v(max(from, to))
                val alpha = if (probe == null || probe == c) 1f else 0.45f
                if (horizontal) {
                    drawRect(color.copy(alpha = alpha), Offset(left + a * (right - left), top + offset), Size((b - a) * (right - left), width))
                } else {
                    drawRect(color.copy(alpha = alpha), Offset(left + offset, bottom - b * (bottom - top)), Size(width, (b - a) * (bottom - top)))
                }
            }
            if (horizontal) {
                label(measurer, categories[c].take(14), Offset(left - 4.dp.toPx(), top + c * slot + slot / 2), ink, alignEnd = true)
            }
        }
        if (!horizontal) {
            label(measurer, categories.first().take(16), Offset(left, size.height - 8.dp.toPx()), ink)
            if (n > 1) label(measurer, categories.last().take(16), Offset(right, size.height - 8.dp.toPx()), ink, alignEnd = true)
        }
    }
    probe?.let { c ->
        val read = series.joinToString(", ") { (name, _, values) -> "${if (series.size > 1) "$name " else ""}${ChartCard.format(values.getOrElse(c) { 0.0 })}" }
        Text(stringResource(R.string.label_value, categories[c], read), style = MaterialTheme.typography.bodySmall)
    }
}

/** A pie, with each slice's share; touch a slice to read it. */
@Composable
private fun PieChart(card: ChartCard) {
    val values = card.series[0].values.map { it ?: 0.0 }
    val total = values.sum()
    var probe by remember(card) { mutableStateOf<Int?>(null) }
    Row(verticalAlignment = Alignment.CenterVertically) {
        Canvas(
            Modifier.size(160.dp).pointerInput(card) {
                detectTapGestures { pos ->
                    val c = Offset(size.width / 2f, size.height / 2f)
                    val d = pos - c
                    if (sqrt(d.x * d.x + d.y * d.y) > size.width / 2f) return@detectTapGestures
                    var angle = Math.toDegrees(atan2(d.y.toDouble(), d.x.toDouble())) + 90
                    if (angle < 0) angle += 360
                    var acc = 0.0
                    val hit = values.indexOfFirst { acc += it / total * 360; angle <= acc }
                    probe = if (probe == hit) null else hit
                }
            },
        ) {
            var start = -90f
            values.forEachIndexed { i, v ->
                val sweep = (v / total * 360).toFloat()
                val color = seriesColor(i)
                val grow = if (probe == i) 4.dp.toPx() else 0f
                val mid = Math.toRadians((start + sweep / 2).toDouble())
                drawArc(
                    color, start, sweep, useCenter = true,
                    topLeft = Offset((cos(mid) * grow).toFloat(), (sin(mid) * grow).toFloat()),
                    size = size,
                )
                start += sweep
            }
        }
        Spacer(Modifier.width(16.dp))
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            card.labels.forEachIndexed { i, l ->
                Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.alpha(if (probe == null || probe == i) 1f else 0.5f)) {
                    Box(Modifier.size(10.dp).clip(CircleShape).background(seriesColor(i)))
                    Spacer(Modifier.width(6.dp))
                    val share = NumberFormat.getPercentInstance().apply { maximumFractionDigits = 0 }.format(values[i] / total)
                    Text("${l.text} $share", style = MaterialTheme.typography.bodySmall)
                }
            }
        }
    }
}

/** The data under the chart, the first rows of it. */
@Composable
private fun DataTable(card: ChartCard, names: List<String>) {
    val maxRows = 200
    val histogram = card.kind == ChartCard.Kind.HISTOGRAM
    val rows: List<List<String>> = if (histogram) {
        val (edges, counts) = card.histogram ?: return
        edges.windowed(2).mapIndexed { b, (a, z) -> listOf("${ChartCard.format(a)}–${ChartCard.format(z)}") + counts.map { "${it[b]}" } }
    } else {
        card.labels.mapIndexed { i, l -> listOf(l.text) + card.series.map { s -> s.values[i]?.let(ChartCard::format) ?: "–" } }
    }
    val header = listOf(card.xLabel ?: if (histogram) stringResource(R.string.chart_bin) else "") + names
    Row(Modifier.horizontalScroll(rememberScrollState())) {
        header.indices.forEach { col ->
            Column(Modifier.padding(end = 14.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Text(header[col], style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold)
                rows.take(maxRows).forEach { Text(it[col], style = MaterialTheme.typography.labelSmall) }
            }
        }
    }
    if (rows.size > maxRows) {
        Text(stringResource(R.string.chart_first_rows, maxRows, rows.size), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
    }
}
