package dev.cedar.cedar.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
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

// Cedar's colours (docs/assets), as iOS/Cedar/App/Theme.swift: the mark's forest green and clay
// on warm paper (light) or deep charcoal (dark).
private val Green = Color(0xFF3A5F4B)
private val GreenDark = Color(0xFF5E9C78)
private val Clay = Color(0xFFB5553A)
private val ClayDark = Color(0xFFE09A7A)
private val Cream = Color(0xFFF3EDE3)

private val Light = lightColorScheme(
    primary = Green,
    onPrimary = Color.White,
    primaryContainer = Color(0xFFDCE8E0),
    onPrimaryContainer = Color(0xFF0F2A1C),
    secondary = Clay,
    onSecondary = Color.White,
    secondaryContainer = Color(0xFFF6E3DC),
    onSecondaryContainer = Color(0xFF4A1E10),
    background = Color(0xFFF7F5F0),
    surface = Color(0xFFF7F5F0),
    surfaceVariant = Color(0xFFEFECE5),
    // Cards and panels: white on the paper.
    surfaceContainerLowest = Color.White,
    surfaceContainerLow = Color(0xFFF3F0EA),
    surfaceContainer = Color.White,
    surfaceContainerHigh = Color(0xFFF3F0EA),
    // Code, chips and the sidebar switch: a step up from a card.
    surfaceContainerHighest = Color(0xFFEFECE5),
    outlineVariant = Color(0xFFE3DFD6),
)

private val Dark = darkColorScheme(
    primary = GreenDark,
    onPrimary = Color.White,
    primaryContainer = Color(0xFF233B2D),
    onPrimaryContainer = Color(0xFFD5E6DB),
    secondary = ClayDark,
    onSecondary = Color(0xFF2A120A),
    secondaryContainer = Color(0xFF3A2219),
    onSecondaryContainer = Color(0xFFF6E3DC),
    background = Color(0xFF111311),
    surface = Color(0xFF111311),
    surfaceVariant = Color(0xFF252925),
    surfaceContainerLowest = Color(0xFF0C0E0C),
    surfaceContainerLow = Color(0xFF161916),
    surfaceContainer = Color(0xFF1B1E1B),
    surfaceContainerHigh = Color(0xFF202420),
    surfaceContainerHighest = Color(0xFF252925),
    outlineVariant = Color(0xFF2E332E),
    onSurface = Cream,
    onBackground = Cream,
)

/** Light and dark, following the system, in Cedar's colours. */
@Composable
fun CedarTheme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = if (isSystemInDarkTheme()) Dark else Light, content = content)
}

/** The colours the cards and turns use beyond Material's roles. */
object CedarColors {
    val green = Color(0xFF2E9D5B)
    val red = Color(0xFFD14343)
    val orange = Color(0xFFE08A1E)
    val yellow = Color(0xFFE8B416)

    val secondaryText: Color
        @Composable @ReadOnlyComposable get() = MaterialTheme.colorScheme.onSurfaceVariant

    /** The mark's clay: what needs the user (answers, approvals) and unread counts. */
    val clay: Color
        @Composable @ReadOnlyComposable get() = MaterialTheme.colorScheme.secondary

    /** The mark's bars: the green on paper, cream on charcoal. */
    val markInk: Color
        @Composable @ReadOnlyComposable get() = if (MaterialTheme.colorScheme.background.luminance() < 0.5f) Cream else Green
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
    val clay = CedarColors.clay
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

/** A primary action: the brand green, tall and softly rounded, as the iOS app's `ProminentButtonStyle`. */
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
        content = content,
    )
}
