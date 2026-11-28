package dev.cedar.cedar.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.ui.Alignment
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import dev.cedar.cedar.R
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.unit.dp

// Cedar's colours, as the web's Today page and iOS/Cedar/App/Theme.swift: a neutral page with
// white cards (light) or a green-black one (dark), one hue family for what Cedar does (a dark
// green for links, selection and toggles, lime on the darkest green for the action of a view),
// and amber for what needs the person. Red is for what failed, never for attention. The mark
// keeps its own forest green and clay: that is the logo, not the interface.
private val MarkGreen = Color(0xFF3A5F4B)
private val MarkClay = Color(0xFFB5553A)
private val MarkClayDark = Color(0xFFE09A7A)
private val Paper = Color(0xFFEDEFE8)

private val Lime = Color(0xFFC8F169)
private val Amber = Color(0xFFF2C14E)
private val AmberInk = Color(0xFF1F1600)
/** The darkest green: the fill of a view's main action in light mode, and the Now block. */
private val Night = Color(0xFF0F1F17)

private val Light = lightColorScheme(
    // Links, selection, focus, toggles; a plain filled button is this with lime text.
    primary = Color(0xFF1F5A40),
    onPrimary = Lime,
    primaryContainer = Color(0xFFE4EFD9),
    onPrimaryContainer = Night,
    // What needs the person: dark amber on the page, on a pale amber ground.
    secondary = Color(0xFF6B4A00),
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFFFF1CC),
    onSecondaryContainer = Color(0xFF6B4A00),
    background = Color(0xFFF7F7F4),
    onBackground = Color(0xFF121815),
    surface = Color(0xFFF7F7F4),
    onSurface = Color(0xFF121815),
    surfaceVariant = Color(0xFFECEDE8),
    onSurfaceVariant = Color(0xFF5B635D),
    // Cards and panels: white on the page.
    surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFF1F2EE),
    surfaceContainer = Color.White,
    surfaceContainerHigh = Color(0xFFF1F2EE),
    // Code, chips and the sidebar switch: a step up from a card.
    surfaceContainerHighest = Color(0xFFECEDE8),
    outline = Color(0xFFC4C9C2),
    outlineVariant = Color(0xFFE2E4DE),
)

private val Dark = darkColorScheme(
    primary = Lime,
    onPrimary = Color(0xFF0F1411),
    primaryContainer = Color(0xFF22361A),
    onPrimaryContainer = Lime,
    secondary = Amber,
    onSecondary = AmberInk,
    secondaryContainer = Color(0xFF3A2F10),
    onSecondaryContainer = Amber,
    background = Color(0xFF0F1411),
    onBackground = Paper,
    surface = Color(0xFF0F1411),
    onSurface = Paper,
    surfaceVariant = Color(0xFF222B26),
    onSurfaceVariant = Color(0xFF9AA39B),
    surfaceContainerLowest = Color(0xFF0B0F0D),
    surfaceContainerLow = Color(0xFF141A16),
    surfaceContainer = Color(0xFF171E1A),
    surfaceContainerHigh = Color(0xFF1C2420),
    surfaceContainerHighest = Color(0xFF222B26),
    outline = Color(0xFF3A4A3F),
    outlineVariant = Color(0xFF26302A),
)

/** Light and dark, following the system, in Cedar's colours. */
@Composable
fun CedarTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) Dark else Light, content = content)
}

// -- Today's own colours -------------------------------------------------------------------------
//
// The Today screens (the river, the ask sheet, an outcome) use the app's colours, with two things
// of their own in light mode: their buttons are the darkest green with lime text, and what is
// now sits in one dark-green block drawn in the dark theme's colours. Everything resolves through
// MaterialTheme, so the cards the chat shares (a held call, a follow-up) take the colours of
// wherever they are put.

/** The Today page in light mode: the app's, with the darkest green for its actions. */
private val TodayLight = Light.copy(primary = Night, onPrimary = Lime)

/** What is now, in light mode: the dark-green block. */
private val TodayNow = darkColorScheme(
    primary = Lime,
    onPrimary = Night,
    primaryContainer = Color(0xFF22361A),
    onPrimaryContainer = Lime,
    secondary = Amber,
    onSecondary = AmberInk,
    secondaryContainer = Color(0xFF3A2F10),
    onSecondaryContainer = Amber,
    background = Night,
    onBackground = Paper,
    surface = Night,
    onSurface = Paper,
    surfaceVariant = Color(0xFF22342A),
    onSurfaceVariant = Color(0xFF9AA39B),
    surfaceContainerLowest = Color(0xFF0B1711),
    surfaceContainerLow = Color(0xFF17291F),
    surfaceContainer = Color(0xFF17291F),
    surfaceContainerHigh = Color(0xFF1D3126),
    surfaceContainerHighest = Color(0xFF22342A),
    outline = Color(0xFF3F5A4B),
    outlineVariant = Color(0xFF2E4638),
)

/** The Today screens' colours over the app's, with the text colour that goes with them. */
@Composable
fun TodayTheme(content: @Composable () -> Unit) {
    val scheme = if (isSystemInDarkTheme()) Dark else TodayLight
    MaterialTheme(colorScheme = scheme) {
        CompositionLocalProvider(LocalContentColor provides scheme.onSurface, content = content)
    }
}

/**
 * The block what is now sits in: dark green in light mode, with everything inside it (the shared
 * cards too) drawn in the block's own colours; in dark mode a slightly raised surface.
 */
@Composable
fun TodayNowBlock(modifier: Modifier = Modifier, content: @Composable ColumnScope.() -> Unit) {
    val dark = isSystemInDarkTheme()
    val scheme = if (dark) Dark else TodayNow
    MaterialTheme(colorScheme = scheme) {
        CompositionLocalProvider(LocalContentColor provides scheme.onSurface) {
            Column(
                modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(24.dp))
                    .background(if (dark) scheme.surfaceContainerLow else scheme.background)
                    .padding(vertical = 10.dp),
                content = content,
            )
        }
    }
}

/** A dashed outline, for what hasn't happened yet. */
fun Modifier.dashedOutline(color: Color, radius: Dp = 16.dp): Modifier = drawBehind {
    val stroke = 1.dp.toPx()
    drawRoundRect(
        color,
        topLeft = Offset(stroke / 2, stroke / 2),
        size = Size(size.width - stroke, size.height - stroke),
        cornerRadius = CornerRadius(radius.toPx()),
        style = Stroke(stroke, pathEffect = PathEffect.dashPathEffect(floatArrayOf(4.dp.toPx(), 4.dp.toPx()))),
    )
}

/** The colours the cards and turns use beyond Material's roles. */
object CedarColors {
    val green = Color(0xFF2E9D5B)
    val red = Color(0xFFD14343)
    val orange = Color(0xFFE08A1E)
    val yellow = Color(0xFFE8B416)

    val secondaryText: Color
        @Composable @ReadOnlyComposable get() = MaterialTheme.colorScheme.onSurfaceVariant

    /** What needs the user (answers, approvals, held calls) and unread counts: amber, as text or a tint. */
    val clay: Color
        @Composable @ReadOnlyComposable get() = MaterialTheme.colorScheme.secondary

    /** The amber as a fill (a badge, a dot), and the ink that goes on it. */
    val attentionFill: Color get() = Amber
    val onAttentionFill: Color get() = AmberInk

    /** The fill of a view's main action, and its text: lime on the darkest green, or the reverse on a dark ground. */
    val actionFill: Color
        @Composable @ReadOnlyComposable get() = if (MaterialTheme.colorScheme.background.luminance() < 0.5f) Lime else Night
    val onActionFill: Color
        @Composable @ReadOnlyComposable get() = if (MaterialTheme.colorScheme.background.luminance() < 0.5f) Night else Lime

    /** What the person wrote, in the chat: the darkest green (a raised green in the dark), with paper-white text. */
    val bubble: Color
        @Composable @ReadOnlyComposable get() = if (MaterialTheme.colorScheme.background.luminance() < 0.5f) Color(0xFF22361A) else Night
    val onBubble: Color get() = Paper

    /** The mark's bars: its own green on a light ground, paper on a dark one. */
    val markInk: Color
        @Composable @ReadOnlyComposable get() = if (MaterialTheme.colorScheme.background.luminance() < 0.5f) Paper else MarkGreen

    /** The mark's dash: its own clay, which is the logo's and not the interface's amber. */
    val markDash: Color
        @Composable @ReadOnlyComposable get() = if (MaterialTheme.colorScheme.background.luminance() < 0.5f) MarkClayDark else MarkClay
}

/** The frame tool calls and cards sit in, as the iOS app's `cardStyle()`. */
@Composable
fun Modifier.cardFrame(): Modifier = this
    .fillMaxWidth()
    .surfaceCard()
    .padding(14.dp)

/** A card: the card surface, a hairline and a soft shadow, as the iOS app's `surfaceCard()`. */
@Composable
fun Modifier.surfaceCard(
    radius: Dp = 16.dp,
    fill: Color = MaterialTheme.colorScheme.surfaceContainer,
    stroke: Color = MaterialTheme.colorScheme.outlineVariant,
): Modifier {
    val shape = RoundedCornerShape(radius)
    return this
        .shadow(2.dp, shape, ambientColor = Color.Black.copy(alpha = 0.06f), spotColor = Color.Black.copy(alpha = 0.06f))
        .clip(shape)
        .background(fill)
        .border(1.dp, stroke, shape)
}

/** Cedar's mark: a "c" of four bars with the clay dash, drawn from docs/assets/cedar-mark.svg. */
@Composable
fun CedarMark(size: Dp = 28.dp) {
    val ink = CedarColors.markInk
    val clay = CedarColors.markDash
    Canvas(Modifier.size(size)) {
        val s = this.size.width / 48f
        fun bar(x: Float, y: Float, width: Float, color: Color) = drawRoundRect(
            color, topLeft = Offset(x * s, y * s), size = Size(width * s, 6 * s), cornerRadius = CornerRadius(2 * s),
        )
        bar(6f, 6f, 36f, ink)
        bar(6f, 16f, 12f, ink)
        bar(6f, 26f, 12f, ink)
        bar(6f, 36f, 36f, ink)
        bar(33f, 21f, 9f, clay)
    }
}

/** The mark and the name, as the top bar shows them. */
@Composable
fun CedarWordmark(size: TextUnit = 22.sp) {
    val brand = stringResource(R.string.brand)
    Row(
        Modifier.semantics(mergeDescendants = true) { heading() },
        verticalAlignment = Alignment.CenterVertically,
    ) {
        CedarMark(with(LocalDensity.current) { (size * 1.15f).toDp() })
        Spacer(Modifier.width(with(LocalDensity.current) { (size * 0.3f).toDp() }))
        Text(
            brand,
            fontSize = size,
            fontWeight = FontWeight.SemiBold,
            letterSpacing = (-0.4).sp,
            color = CedarColors.markInk,
        )
    }
}

/** The mark on a soft tile, for welcomes and empty screens. */
@Composable
fun CedarBadge(size: Dp = 72.dp) {
    Box(
        Modifier.size(size).surfaceCard(radius = size * 0.28f),
        contentAlignment = Alignment.Center,
    ) { CedarMark(size * 0.62f) }
}

/** A row's leading tile: what the row is, or a spinner while its agent runs. */
@Composable
fun RowIcon(icon: ImageVector, tint: Color = MaterialTheme.colorScheme.primary, running: Boolean = false) {
    Box(
        Modifier.size(42.dp).clip(RoundedCornerShape(12.dp)).background(tint.copy(alpha = 0.13f)),
        contentAlignment = Alignment.Center,
    ) {
        if (running) {
            CircularProgressIndicator(Modifier.size(18.dp), color = tint, strokeWidth = 2.dp)
        } else {
            Icon(icon, null, Modifier.size(20.dp), tint = tint)
        }
    }
}

/** A view's main action: lime on the darkest green, tall and softly rounded, as the iOS app's `ProminentButtonStyle`. */
@Composable
fun ProminentButton(
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    content: @Composable RowScope.() -> Unit,
) {
    Button(
        onClick = onClick,
        modifier = modifier.heightIn(min = 52.dp),
        enabled = enabled,
        shape = RoundedCornerShape(14.dp),
        colors = ButtonDefaults.buttonColors(containerColor = CedarColors.actionFill, contentColor = CedarColors.onActionFill),
        content = content,
    )
}
