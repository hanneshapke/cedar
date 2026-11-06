package dev.cedar.cedar.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.BarChart
import androidx.compose.material.icons.outlined.Bookmark
import androidx.compose.material.icons.outlined.BookmarkBorder
import androidx.compose.material.icons.outlined.CalendarMonth
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Checklist
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.Email
import androidx.compose.material.icons.outlined.FindInPage
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.NotificationsActive
import androidx.compose.material.icons.outlined.Place
import androidx.compose.material.icons.outlined.RadioButtonChecked
import androidx.compose.material.icons.outlined.RadioButtonUnchecked
import androidx.compose.material.icons.outlined.Restaurant
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.Smartphone
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.Update
import androidx.compose.material.icons.outlined.WbSunny
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.cedar.cedar.R
import kotlin.math.min

/**
 * The pictures the feature tour shows, as the iOS app's TourIllustrations.swift: small mock-ups of
 * the app's own screens, drawn from the theme's shapes and Material icons so they match the real
 * thing in both themes and need no image assets. Each is laid out on a 320 × 250 dp stage and
 * scaled to the room it gets.
 */
enum class TourIllustration {
    SERVER, WORK, CARDS, PLANS, SCHEDULES, MEMORY, CONNECT;

    /** [shown]: whether the elements are shown (they fade and rise in, one after another, when a page comes up). */
    @Composable
    fun Draw(shown: Boolean) {
        when (this) {
            SERVER -> ServerPicture(shown)
            WORK -> WorkPicture(shown)
            CARDS -> CardsPicture(shown)
            PLANS -> PlansPicture(shown)
            SCHEDULES -> SchedulesPicture(shown)
            MEMORY -> MemoryPicture(shown)
            CONNECT -> ConnectPicture(shown)
        }
    }
}

private val STAGE_WIDTH = 320.dp
private val STAGE_HEIGHT = 250.dp

/** A fixed 320 × 250 dp drawing, scaled to fit the space it is given. */
@Composable
private fun Stage(content: @Composable () -> Unit) {
    BoxWithConstraints(Modifier.fillMaxSize().clearAndSetSemantics {}, contentAlignment = Alignment.Center) {
        val scale = min(maxWidth / STAGE_WIDTH, maxHeight / STAGE_HEIGHT)
        Box(
            Modifier.requiredSize(STAGE_WIDTH, STAGE_HEIGHT).graphicsLayer(scaleX = scale, scaleY = scale),
            contentAlignment = Alignment.Center,
        ) { content() }
    }
}

/** Fades and rises in as the [index]th element of its picture. */
@Composable
private fun Reveal(shown: Boolean, index: Int, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    val progress by animateFloatAsState(if (shown) 1f else 0f, tween(550, delayMillis = index * 100), label = "reveal")
    Box(modifier.alpha(progress).offset(y = (14 * (1 - progress)).dp)) { content() }
}

// Parts

private val surface: Color @Composable get() = MaterialTheme.colorScheme.surfaceContainer
private val line: Color @Composable get() = MaterialTheme.colorScheme.outlineVariant
private val raised: Color @Composable get() = MaterialTheme.colorScheme.surfaceContainerHighest
private val green: Color @Composable get() = MaterialTheme.colorScheme.primary

/** A small card on the stage: the surface, a hairline and a soft shadow, like the app's cards. */
@Composable
private fun MiniCard(width: Dp? = null, padding: Dp = 12.dp, content: @Composable () -> Unit) {
    Box(
        Modifier.then(if (width != null) Modifier.width(width) else Modifier).surfaceCard(radius = 14.dp).padding(padding),
        contentAlignment = Alignment.CenterStart,
    ) { content() }
}

/** A line of text stood in for by a soft bar. */
@Composable
private fun TextLine(width: Dp, tint: Color = raised) {
    Box(Modifier.size(width, 8.dp).clip(RoundedCornerShape(50)).background(tint))
}

/** A chat bubble with real words. */
@Composable
private fun Bubble(text: String, mine: Boolean = false) {
    val shape = RoundedCornerShape(14.dp)
    Text(
        text,
        fontSize = 12.sp,
        fontWeight = FontWeight.Medium,
        color = if (mine) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
        modifier = Modifier
            .clip(shape)
            .background(if (mine) green else surface)
            .then(if (mine) Modifier else Modifier.border(1.dp, line, shape))
            .padding(horizontal = 11.dp, vertical = 8.dp),
    )
}

/** A tool call's row, as the chat shows it: the symbol, the name, the time it took and a check. */
@Composable
private fun ToolRow(icon: ImageVector, name: String, duration: String) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
        Box(Modifier.size(18.dp).clip(RoundedCornerShape(5.dp)).background(green.copy(alpha = 0.13f)), contentAlignment = Alignment.Center) {
            Icon(icon, null, Modifier.size(12.dp), tint = green)
        }
        Text(name, fontSize = 11.sp, fontWeight = FontWeight.Medium, fontFamily = FontFamily.Monospace)
        Spacer(Modifier.weight(1f))
        Text(duration, fontSize = 10.sp, color = CedarColors.secondaryText)
        Icon(Icons.Outlined.CheckCircle, null, Modifier.size(12.dp), tint = green)
    }
}

/** A rounded tile with an icon on it, for the server page. */
@Composable
private fun Tile(icon: ImageVector, tint: Color, size: Dp = 56.dp) {
    Box(Modifier.size(size).surfaceCard(radius = size * 0.28f), contentAlignment = Alignment.Center) {
        Icon(icon, null, Modifier.size(size * 0.46f), tint = tint)
    }
}

/** A dashed link between two tiles. */
@Composable
private fun Wire(modifier: Modifier = Modifier) {
    val color = green.copy(alpha = 0.6f)
    Canvas(modifier.height(2.dp)) {
        drawLine(
            color, Offset(0f, size.height / 2), Offset(size.width, size.height / 2),
            strokeWidth = 2.dp.toPx(), cap = StrokeCap.Round,
            pathEffect = PathEffect.dashPathEffect(floatArrayOf(1.dp.toPx(), 6.dp.toPx())),
        )
    }
}

/** A caption under a tile. */
@Composable
private fun Caption(text: String) {
    Text(text, fontSize = 11.sp, fontWeight = FontWeight.Medium, color = CedarColors.secondaryText, maxLines = 1)
}

/** A small round icon badge, like the tool icons under the server. */
@Composable
private fun RoundIcon(icon: ImageVector, size: Dp = 34.dp, tint: Color = green) {
    Box(Modifier.size(size).clip(CircleShape).background(tint.copy(alpha = 0.12f)), contentAlignment = Alignment.Center) {
        Icon(icon, null, Modifier.size(size * 0.45f), tint = tint)
    }
}

// Pictures

/** The phone, the server with cedar on it, and the sandbox, wired together; a padlock on the server. */
@Composable
private fun ServerPicture(shown: Boolean) {
    Stage {
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(26.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Reveal(shown, 0) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Tile(Icons.Outlined.Smartphone, CedarColors.markInk)
                        Caption(stringResource(R.string.tour_this_app))
                    }
                }
                Reveal(shown, 1, Modifier.padding(horizontal = 6.dp).padding(bottom = 22.dp)) { Wire(Modifier.width(26.dp)) }
                Reveal(shown, 2) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Box {
                            Box(Modifier.size(78.dp).surfaceCard(radius = 22.dp), contentAlignment = Alignment.Center) { CedarMark(44.dp) }
                            Box(
                                Modifier.align(Alignment.TopEnd).offset(x = 6.dp, y = (-6).dp).size(24.dp).clip(CircleShape).background(green),
                                contentAlignment = Alignment.Center,
                            ) { Icon(Icons.Outlined.Lock, null, Modifier.size(12.dp), tint = MaterialTheme.colorScheme.onPrimary) }
                        }
                        Caption(stringResource(R.string.tour_your_server))
                    }
                }
                Reveal(shown, 3, Modifier.padding(horizontal = 6.dp).padding(bottom = 22.dp)) { Wire(Modifier.width(26.dp)) }
                Reveal(shown, 4) {
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Tile(Icons.Outlined.Terminal, CedarColors.clay)
                        Caption(stringResource(R.string.tour_sandbox))
                    }
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                listOf(Icons.Outlined.Language, Icons.Outlined.FindInPage, Icons.Outlined.Email, Icons.Outlined.CalendarMonth, Icons.Outlined.BarChart)
                    .forEachIndexed { i, icon -> Reveal(shown, 5 + i) { RoundIcon(icon) } }
            }
        }
    }
}

/** A question, and the reply taking shape: the folded reasoning, two tool calls, and text. */
@Composable
private fun WorkPicture(shown: Boolean) {
    Stage {
        Column(Modifier.padding(horizontal = 10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Reveal(shown, 0, Modifier.fillMaxWidth()) {
                Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) { Bubble(stringResource(R.string.tour_ask), mine = true) }
            }
            MiniCard(width = 260.dp, padding = 10.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Reveal(shown, 1) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Icon(Icons.Outlined.ChevronRight, null, Modifier.size(12.dp), tint = CedarColors.secondaryText)
                            Text(stringResource(R.string.reasoning), fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = CedarColors.secondaryText)
                            Text("· 4 s", fontSize = 11.sp, color = CedarColors.secondaryText)
                        }
                    }
                    Reveal(shown, 2, Modifier.fillMaxWidth()) { ToolRow(Icons.Outlined.Search, "web_search", "1.8 s") }
                    Reveal(shown, 3, Modifier.fillMaxWidth()) { ToolRow(Icons.Outlined.Code, "run_python", "0.6 s") }
                    Reveal(shown, 4) {
                        Column(Modifier.padding(top = 2.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            TextLine(220.dp)
                            TextLine(190.dp)
                            TextLine(120.dp)
                        }
                    }
                    Reveal(shown, 5) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                            CircularProgressIndicator(Modifier.size(10.dp), strokeWidth = 1.5.dp)
                            Text(stringResource(R.string.working), fontSize = 10.sp, color = CedarColors.secondaryText)
                        }
                    }
                }
            }
        }
    }
}

/** Four of the native cards: the weather, a chart, a recipe and a route. */
@Composable
private fun CardsPicture(shown: Boolean) {
    Stage {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Reveal(shown, 0) {
                    MiniCard(width = 150.dp) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                            Icon(Icons.Outlined.WbSunny, null, Modifier.size(30.dp), tint = CedarColors.orange)
                            Column {
                                Text("21°", fontSize = 22.sp, fontWeight = FontWeight.Bold)
                                Text(stringResource(R.string.tour_sunny), fontSize = 11.sp, color = CedarColors.secondaryText)
                            }
                        }
                    }
                }
                Reveal(shown, 1) {
                    MiniCard(width = 150.dp) {
                        Row(Modifier.height(52.dp), verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
                            listOf(22, 34, 28, 44, 52).forEachIndexed { i, h ->
                                Box(Modifier.size(14.dp, h.dp).clip(RoundedCornerShape(3.dp)).background(if (i == 4) CedarColors.clay else green))
                            }
                        }
                    }
                }
            }
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                Reveal(shown, 2) {
                    MiniCard(width = 150.dp) {
                        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                                Icon(Icons.Outlined.Restaurant, null, Modifier.size(14.dp), tint = CedarColors.clay)
                                Text(stringResource(R.string.tour_recipe), fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                            }
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                                repeat(3) {
                                    Box(Modifier.size(6.dp).clip(CircleShape).background(green))
                                    TextLine(24.dp)
                                }
                            }
                            TextLine(100.dp)
                        }
                    }
                }
                Reveal(shown, 3) {
                    MiniCard(width = 150.dp, padding = 0.dp) {
                        Box(Modifier.fillMaxWidth().height(70.dp).background(green.copy(alpha = 0.1f))) {
                            val route = green
                            Canvas(Modifier.fillMaxSize().padding(16.dp)) {
                                val path = Path().apply {
                                    moveTo(0f, size.height)
                                    cubicTo(size.width * 0.2f, 0f, size.width * 0.8f, size.height, size.width, 0f)
                                }
                                drawPath(path, route, style = Stroke(width = 3.dp.toPx(), cap = StrokeCap.Round, join = StrokeJoin.Round))
                            }
                            Icon(Icons.Outlined.Place, null, Modifier.align(Alignment.Center).offset(x = 48.dp, y = (-16).dp).size(18.dp), tint = CedarColors.clay)
                        }
                    }
                }
            }
        }
    }
}

/** A plan with its steps and the buttons that agree to it, and a question with its choices. */
@Composable
private fun PlansPicture(shown: Boolean) {
    Stage {
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            MiniCard(width = 270.dp) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Reveal(shown, 0, Modifier.fillMaxWidth()) {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Icon(Icons.Outlined.Checklist, null, Modifier.size(14.dp), tint = green)
                            Text(stringResource(R.string.plan), fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                            Spacer(Modifier.weight(1f))
                            Text(
                                stringResource(R.string.tour_waiting_for_you), fontSize = 10.sp, fontWeight = FontWeight.SemiBold, color = CedarColors.clay,
                                modifier = Modifier.clip(RoundedCornerShape(50)).background(CedarColors.clay.copy(alpha = 0.15f)).padding(horizontal = 7.dp, vertical = 3.dp),
                            )
                        }
                    }
                    Step(shown, 1, stringResource(R.string.tour_step_read))
                    Step(shown, 2, stringResource(R.string.tour_step_compute))
                    Step(shown, 3, stringResource(R.string.tour_step_draw))
                    Reveal(shown, 4) {
                        Row(Modifier.padding(top = 2.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Pill(stringResource(R.string.go_ahead), MaterialTheme.colorScheme.onPrimary, green)
                            Pill(stringResource(R.string.stop), CedarColors.clay, CedarColors.clay.copy(alpha = 0.12f))
                        }
                    }
                }
            }
            Reveal(shown, 5) {
                MiniCard(width = 270.dp) {
                    Column(verticalArrangement = Arrangement.spacedBy(7.dp)) {
                        Text(stringResource(R.string.tour_question), fontSize = 11.sp, fontWeight = FontWeight.SemiBold)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Choice(stringResource(R.string.tour_calendar_year), on = true)
                            Choice(stringResource(R.string.tour_fiscal_year), on = false)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Step(shown: Boolean, index: Int, text: String) {
    Reveal(shown, index) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(7.dp)) {
            Box(Modifier.size(16.dp).clip(CircleShape).background(green.copy(alpha = 0.13f)), contentAlignment = Alignment.Center) {
                Text("$index", fontSize = 9.sp, fontWeight = FontWeight.Bold, color = green)
            }
            Text(text, fontSize = 11.sp)
        }
    }
}

@Composable
private fun Pill(text: String, color: Color, fill: Color) {
    Text(
        text, fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = color,
        modifier = Modifier.clip(RoundedCornerShape(50)).background(fill).padding(horizontal = 12.dp, vertical = 6.dp),
    )
}

@Composable
private fun Choice(text: String, on: Boolean) {
    Row(
        Modifier.clip(RoundedCornerShape(50)).background(raised).padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(5.dp),
    ) {
        Icon(if (on) Icons.Outlined.RadioButtonChecked else Icons.Outlined.RadioButtonUnchecked, null, Modifier.size(12.dp),
            tint = if (on) green else CedarColors.secondaryText)
        Text(text, fontSize = 11.sp, fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal)
    }
}

/** Two scheduled agents in the sidebar, and the notification a finished run sends. */
@Composable
private fun SchedulesPicture(shown: Boolean) {
    Stage {
        Column(verticalArrangement = Arrangement.spacedBy(12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Reveal(shown, 0) {
                Row(
                    Modifier.width(280.dp).surfaceCard(radius = 16.dp, fill = MaterialTheme.colorScheme.surfaceContainerHigh).padding(10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Box(Modifier.size(32.dp).clip(RoundedCornerShape(9.dp)).background(CedarColors.clay.copy(alpha = 0.13f)), contentAlignment = Alignment.Center) {
                        Icon(Icons.Outlined.NotificationsActive, null, Modifier.size(18.dp), tint = CedarColors.clay)
                    }
                    Column(Modifier.weight(1f)) {
                        Text(stringResource(R.string.tour_briefing_ready), fontSize = 12.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(stringResource(R.string.tour_briefing_text), fontSize = 11.sp, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    Text(stringResource(R.string.tour_now), fontSize = 10.sp, color = CedarColors.secondaryText)
                }
            }
            MiniCard(width = 280.dp, padding = 6.dp) {
                Column {
                    Reveal(shown, 1) { ScheduleMock(stringResource(R.string.tour_morning_briefing), stringResource(R.string.tour_daily_at_7), unread = true) }
                    HorizontalDivider(Modifier.padding(horizontal = 8.dp))
                    Reveal(shown, 2) { ScheduleMock(stringResource(R.string.tour_competitor_scan), stringResource(R.string.tour_mondays), unread = false) }
                    HorizontalDivider(Modifier.padding(horizontal = 8.dp))
                    Reveal(shown, 3) { ScheduleMock(stringResource(R.string.tour_inbox_summary), stringResource(R.string.tour_every_evening), unread = false) }
                }
            }
        }
    }
}

@Composable
private fun ScheduleMock(title: String, cadence: String, unread: Boolean) {
    Row(Modifier.fillMaxWidth().padding(8.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Box(Modifier.size(30.dp).clip(RoundedCornerShape(9.dp)).background(green.copy(alpha = 0.13f)), contentAlignment = Alignment.Center) {
            Icon(Icons.Outlined.Update, null, Modifier.size(16.dp), tint = green)
        }
        Column(Modifier.weight(1f)) {
            Text(title, fontSize = 12.sp, fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(cadence, fontSize = 10.sp, color = CedarColors.secondaryText)
        }
        if (unread) UnreadDot(true, size = 8)
    }
}

/** What Cedar remembers, as chips, with a lock on the server they stay on. */
@Composable
private fun MemoryPicture(shown: Boolean) {
    Stage {
        Column(verticalArrangement = Arrangement.spacedBy(16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
            Reveal(shown, 0) {
                Box(Modifier.size(84.dp), contentAlignment = Alignment.Center) {
                    Box(Modifier.size(84.dp).clip(CircleShape).background(green.copy(alpha = 0.12f)))
                    Icon(Icons.Outlined.Bookmark, null, Modifier.size(40.dp), tint = green)
                    Box(
                        Modifier.align(Alignment.Center).offset(x = 32.dp, y = 30.dp).size(26.dp).clip(CircleShape).background(CedarColors.clay),
                        contentAlignment = Alignment.Center,
                    ) { Icon(Icons.Outlined.Lock, null, Modifier.size(12.dp), tint = MaterialTheme.colorScheme.onSecondary) }
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                MemoryChip(shown, 1, stringResource(R.string.tour_memory_units))
                MemoryChip(shown, 2, stringResource(R.string.tour_memory_app))
                MemoryChip(shown, 3, stringResource(R.string.tour_memory_trip))
            }
        }
    }
}

@Composable
private fun MemoryChip(shown: Boolean, index: Int, text: String) {
    Reveal(shown, index) {
        Row(
            Modifier.width(250.dp).surfaceCard(radius = 12.dp).padding(horizontal = 12.dp, vertical = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Icon(Icons.Outlined.BookmarkBorder, null, Modifier.size(13.dp), tint = green)
            Text(text, fontSize = 12.sp, fontWeight = FontWeight.Medium, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
            Icon(Icons.Outlined.Close, null, Modifier.size(11.dp), tint = CedarColors.secondaryText)
        }
    }
}

/** A QR code in the viewfinder's corners, and the fields it fills in. */
@Composable
private fun ConnectPicture(shown: Boolean) {
    Stage {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(22.dp)) {
            Reveal(shown, 0) {
                Box(Modifier.size(136.dp), contentAlignment = Alignment.Center) {
                    QRCodeMock(Modifier.size(96.dp))
                    val corner = green
                    Canvas(Modifier.fillMaxSize()) {
                        val l = 22.dp.toPx()
                        val r = 10.dp.toPx()
                        val w = size.width
                        val h = size.height
                        val path = Path().apply {
                            moveTo(0f, l); lineTo(0f, r); quadraticTo(0f, 0f, r, 0f); lineTo(l, 0f)
                            moveTo(w - l, 0f); lineTo(w - r, 0f); quadraticTo(w, 0f, w, r); lineTo(w, l)
                            moveTo(w, h - l); lineTo(w, h - r); quadraticTo(w, h, w - r, h); lineTo(w - l, h)
                            moveTo(l, h); lineTo(r, h); quadraticTo(0f, h, 0f, h - r); lineTo(0f, h - l)
                        }
                        drawPath(path, corner, style = Stroke(width = 4.dp.toPx(), cap = StrokeCap.Round))
                    }
                }
            }
            Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Field(shown, 1, stringResource(R.string.server_url), "https://home.ts.net")
                Field(shown, 2, stringResource(R.string.access_token), "••••••••••")
                Reveal(shown, 3) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(5.dp)) {
                        Icon(Icons.Outlined.CheckCircle, null, Modifier.size(13.dp), tint = green)
                        Text(stringResource(R.string.connected_plain), fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = green)
                    }
                }
            }
        }
    }
}

@Composable
private fun Field(shown: Boolean, index: Int, label: String, value: String) {
    Reveal(shown, index) {
        Column(Modifier.width(150.dp).surfaceCard(radius = 10.dp).padding(horizontal = 10.dp, vertical = 7.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(label, fontSize = 9.sp, color = CedarColors.secondaryText)
            Text(value, fontSize = 12.sp, fontFamily = FontFamily.Monospace, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** A fixed pattern that looks like a QR code (it encodes nothing). */
@Composable
private fun QRCodeMock(modifier: Modifier) {
    val ink = CedarColors.markInk
    val paper = surface
    Canvas(modifier.clip(RoundedCornerShape(8.dp)).background(paper).padding(6.dp)) {
        val n = 13
        val cell = size.width / n
        for (y in 0 until n) for (x in 0 until n) {
            val finder = (x < 5 && y < 5) || (x > 7 && y < 5) || (x < 5 && y > 7)
            val dark = if (finder) {
                val fx = if (x > 7) x - 8 else x
                val fy = if (y > 7) y - 8 else y
                fx == 0 || fx == 4 || fy == 0 || fy == 4 || (fx in 1..3 && fy in 1..3)
            } else {
                (x * 7 + y * 13 + x * y) % 3 == 0
            }
            if (dark) drawRect(ink, Offset(x * cell, y * cell), androidx.compose.ui.geometry.Size(cell, cell))
        }
    }
}
