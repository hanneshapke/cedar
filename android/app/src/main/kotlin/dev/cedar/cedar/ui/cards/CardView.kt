package dev.cedar.cedar.ui.cards

import android.content.Context
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.graphics.drawable.BitmapDrawable
import android.view.ViewGroup
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.OpenInFull
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
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
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.openUrl
import dev.cedar.core.Card
import dev.cedar.core.Coordinate
import dev.cedar.core.MapCard
import dev.cedar.core.Place
import org.osmdroid.tileprovider.tilesource.TileSourceFactory
import org.osmdroid.util.BoundingBox
import org.osmdroid.util.GeoPoint
import org.osmdroid.views.CustomZoomButtonsController
import org.osmdroid.views.MapView
import org.osmdroid.views.overlay.CopyrightOverlay
import org.osmdroid.views.overlay.Marker
import org.osmdroid.views.overlay.Polyline

/** The card a tool's result draws: a map, a recipe, a trip, a stock comparison, products, or a chart. */
@Composable
fun CardView(card: Card) {
    when (card) {
        is Card.Map -> MapCardView(card.card)
        is Card.Recipe -> RecipeCardView(card.card)
        is Card.Trip -> TripCardView(card.card)
        is Card.Stocks -> StocksCardView(card.card)
        is Card.Products -> ProductsCardView(card.card)
        is Card.Chart -> ChartCardView(card.card)
    }
}

/** A small heading over a part of a card. */
@Composable
fun CardLabel(text: String) {
    Text(text.uppercase(), style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold, color = CedarColors.secondaryText)
}

/** A `#rrggbb` colour, as the parsers check it. */
fun hexColor(hex: String): Color? = hex.removePrefix("#").toLongOrNull(16)?.let { Color(0xFF000000 or it) }

/** Black or white, whichever reads better on `color`. */
fun inkOn(color: Color): Color = if (0.2126 * color.red + 0.7152 * color.green + 0.0722 * color.blue > 0.55) Color(0xFF121212) else Color.White

/** The place in a maps app (or OpenStreetMap in the browser). */
fun Place.mapsUrl(): String = "geo:$latitude,$longitude?q=$latitude,$longitude(${android.net.Uri.encode(label)})"

/** A pin on a map: a number or letter in a coloured circle. */
data class Pin(val point: Coordinate, val label: String, val title: String, val color: Color)

/** A line on a map. */
data class Route(val points: List<Coordinate>, val color: Color, val dashed: Boolean = false)

/**
 * A map on OpenStreetMap tiles, with the attribution OSM asks for. In the conversation it doesn't
 * catch the scrolling; a tap opens it full screen, where it moves and zooms.
 */
@Composable
fun CardMap(title: String, pins: List<Pin>, routes: List<Route> = emptyList()) {
    var expanded by remember { mutableStateOf(false) }
    Box(Modifier.fillMaxWidth().height(220.dp).clip(RoundedCornerShape(10.dp))) {
        OsmMap(pins, routes, interactive = false, modifier = Modifier.fillMaxSize())
        Box(Modifier.fillMaxSize().clickable { expanded = true })
        Icon(
            Icons.Outlined.OpenInFull, "Open the map full screen",
            Modifier.align(Alignment.TopEnd).padding(8.dp).size(28.dp).clip(CircleShape)
                .background(MaterialTheme.colorScheme.surface.copy(alpha = 0.85f)).padding(6.dp),
        )
    }
    if (expanded) {
        Dialog(onDismissRequest = { expanded = false }, properties = DialogProperties(usePlatformDefaultWidth = false)) {
            Surface(Modifier.fillMaxSize()) {
                Column {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                        IconButton(onClick = { expanded = false }) { Icon(Icons.Filled.Close, "Close") }
                        Text(title, style = MaterialTheme.typography.titleMedium)
                    }
                    OsmMap(pins, routes, interactive = true, modifier = Modifier.fillMaxSize())
                }
            }
        }
    }
}

@Composable
private fun OsmMap(pins: List<Pin>, routes: List<Route>, interactive: Boolean, modifier: Modifier) {
    val context = LocalContext.current
    AndroidView(
        modifier = modifier,
        factory = { ctx ->
            MapView(ctx).apply {
                layoutParams = ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
                setTileSource(TileSourceFactory.MAPNIK)
                setMultiTouchControls(interactive)
                zoomController.setVisibility(if (interactive) CustomZoomButtonsController.Visibility.SHOW_AND_FADEOUT else CustomZoomButtonsController.Visibility.NEVER)
                isClickable = interactive
                isFocusable = interactive
                if (!interactive) setOnTouchListener { _, _ -> true }
                overlays.add(CopyrightOverlay(ctx))
                for (route in routes) {
                    overlays.add(Polyline(this).apply {
                        setPoints(route.points.map { GeoPoint(it.latitude, it.longitude) })
                        outlinePaint.color = route.color.toArgb()
                        outlinePaint.strokeWidth = 8f
                        if (route.dashed) outlinePaint.pathEffect = android.graphics.DashPathEffect(floatArrayOf(18f, 14f), 0f)
                    })
                }
                for (pin in pins) {
                    overlays.add(Marker(this).apply {
                        position = GeoPoint(pin.point.latitude, pin.point.longitude)
                        title = pin.title
                        icon = pinIcon(context, pin.label, pin.color)
                        setAnchor(Marker.ANCHOR_CENTER, Marker.ANCHOR_CENTER)
                    })
                }
                val points = (pins.map { it.point } + routes.flatMap { it.points }).map { GeoPoint(it.latitude, it.longitude) }
                // A single pin is shown at city scale; several are fitted to the map.
                if (points.size == 1) {
                    controller.setZoom(13.0)
                    controller.setCenter(points[0])
                } else if (points.isNotEmpty()) {
                    addOnFirstLayoutListener { _, _, _, _, _ ->
                        val box = BoundingBox.fromGeoPointsSafe(points)
                        zoomToBoundingBox(box.increaseByScale(1.3f), false, 48)
                    }
                }
            }
        },
        onRelease = { it.onDetach() },
    )
}

/** A round marker with a number or letter, drawn for osmdroid. */
private fun pinIcon(context: Context, label: String, color: Color): BitmapDrawable {
    val density = context.resources.displayMetrics.density
    val size = (26 * density).toInt()
    val bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
    val canvas = Canvas(bitmap)
    val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { this.color = color.toArgb() }
    val ring = Paint(Paint.ANTI_ALIAS_FLAG).apply { this.color = android.graphics.Color.WHITE; style = Paint.Style.STROKE; strokeWidth = 2 * density }
    canvas.drawCircle(size / 2f, size / 2f, size / 2f - density, fill)
    canvas.drawCircle(size / 2f, size / 2f, size / 2f - density, ring)
    val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        this.color = inkOn(color).toArgb()
        textSize = 12 * density
        typeface = Typeface.DEFAULT_BOLD
        textAlign = Paint.Align.CENTER
    }
    canvas.drawText(label, size / 2f, size / 2f - (text.descent() + text.ascent()) / 2, text)
    return BitmapDrawable(context.resources, bitmap)
}

// show_map

@Composable
fun MapCardView(card: MapCard) {
    val context = LocalContext.current
    val accent = MaterialTheme.colorScheme.primary
    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(10.dp)) {
        card.title?.let { Text(it, style = MaterialTheme.typography.titleMedium) }
        CardMap(card.title ?: "Map", card.markers.mapIndexed { i, p -> Pin(p.coordinate, "${i + 1}", p.label, accent) })
        card.markers.forEachIndexed { index, place ->
            Row(
                Modifier.fillMaxWidth().clickable { openUrl(context, place.mapsUrl()) }.padding(vertical = 2.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(22.dp).clip(CircleShape).background(accent), contentAlignment = Alignment.Center) {
                    Text("${index + 1}", style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.Bold, color = inkOn(accent))
                }
                Spacer(Modifier.width(10.dp))
                Text(place.label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
                Text(place.coordinateText, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
            }
        }
        if (card.notFound.isNotEmpty()) {
            Text("Not found: ${card.notFound.joinToString(", ")}", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
    }
}
