package dev.cedar.cedar.ui

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.unit.dp

// Cedar's colours (docs/assets): the green of the icon, cream and clay.
private val Green = Color(0xFF3A5F4B)
private val GreenLight = Color(0xFF8DB89F)
private val Clay = Color(0xFFC9805F)
private val Cream = Color(0xFFF3EDE3)

private val Light = lightColorScheme(
    primary = Green,
    onPrimary = Color.White,
    primaryContainer = Color(0xFFD5E6DB),
    onPrimaryContainer = Color(0xFF0F2A1C),
    secondary = Clay,
    onSecondary = Color.White,
    background = Color(0xFFFBF8F3),
    surface = Color(0xFFFBF8F3),
    surfaceVariant = Color(0xFFEDE7DD),
    surfaceContainer = Color(0xFFF3EEE6),
    surfaceContainerHigh = Color(0xFFEDE7DD),
    surfaceContainerHighest = Color(0xFFE6DFD4),
)

private val Dark = darkColorScheme(
    primary = GreenLight,
    onPrimary = Color(0xFF0F2A1C),
    primaryContainer = Color(0xFF2A4737),
    onPrimaryContainer = Color(0xFFD5E6DB),
    secondary = Color(0xFFE0A98F),
    background = Color(0xFF141815),
    surface = Color(0xFF141815),
    surfaceVariant = Color(0xFF2A302C),
    surfaceContainer = Color(0xFF1C211E),
    surfaceContainerHigh = Color(0xFF242A26),
    surfaceContainerHighest = Color(0xFF2E3530),
    onSurface = Cream,
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
}

/** The frame tool calls and cards sit in, as the iOS app's `cardStyle()`. */
@Composable
fun Modifier.cardFrame(): Modifier = this
    .fillMaxWidth()
    .clip(RoundedCornerShape(14.dp))
    .background(MaterialTheme.colorScheme.surfaceContainer)
    .border(1.dp, MaterialTheme.colorScheme.outlineVariant.copy(alpha = 0.6f), RoundedCornerShape(14.dp))
    .padding(14.dp)
