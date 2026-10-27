package dev.cedar.cedar.ui.cards

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.DirectionsWalk
import androidx.compose.material.icons.outlined.CheckBox
import androidx.compose.material.icons.outlined.CheckBoxOutlineBlank
import androidx.compose.material.icons.outlined.DirectionsBike
import androidx.compose.material.icons.outlined.DirectionsBoat
import androidx.compose.material.icons.outlined.DirectionsBus
import androidx.compose.material.icons.outlined.DirectionsCar
import androidx.compose.material.icons.outlined.DirectionsSubway
import androidx.compose.material.icons.outlined.Flight
import androidx.compose.material.icons.outlined.LocalTaxi
import androidx.compose.material.icons.outlined.Lightbulb
import androidx.compose.material.icons.outlined.Tram
import androidx.compose.material.icons.outlined.Train
import androidx.compose.material.icons.outlined.Circle
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import coil.compose.AsyncImage
import dev.cedar.cedar.R
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.duration
import dev.cedar.cedar.ui.factLabel
import dev.cedar.cedar.ui.legKindLabel
import dev.cedar.cedar.ui.openUrl
import dev.cedar.cedar.ui.shortTime
import dev.cedar.cedar.ui.tripModeLabel
import dev.cedar.cedar.ui.tripTakeLabel
import dev.cedar.core.RecipeCard
import dev.cedar.core.TripCard
import dev.cedar.core.TripStyle
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter

// show_recipe

/** A recipe: the facts, the photo, the ingredients to tick off, the steps to mark done, the tips. */
@Composable
fun RecipeCardView(card: RecipeCard) {
    val context = LocalContext.current
    val have = remember(card) { mutableStateListOf<Int>() }
    val done = remember(card) { mutableStateListOf<Int>() }
    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(card.title, style = MaterialTheme.typography.titleMedium)
        card.description?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText) }
        if (card.facts.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                card.facts.forEach { fact ->
                    Column {
                        CardLabel(factLabel(fact)?.let { stringResource(it) } ?: fact.label)
                        Text(fact.value, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.Medium)
                    }
                }
            }
        }
        card.photo?.let { photo ->
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                AsyncImage(
                    model = photo.url,
                    contentDescription = photo.title ?: card.title,
                    modifier = Modifier.fillMaxWidth().heightIn(max = 240.dp).clip(RoundedCornerShape(10.dp)),
                    contentScale = ContentScale.Crop,
                )
                Text(
                    stringResource(R.string.photo_credit, photo.source),
                    style = MaterialTheme.typography.labelSmall,
                    color = CedarColors.secondaryText,
                    modifier = photo.pageURL?.let { url -> Modifier.clickable { openUrl(context, url) } } ?: Modifier,
                )
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            CardLabel(stringResource(R.string.ingredients))
            card.ingredients.forEachIndexed { index, ingredient ->
                val checked = index in have
                Row(
                    Modifier.fillMaxWidth().clickable { if (checked) have.remove(index) else have.add(index) },
                    verticalAlignment = Alignment.Top,
                ) {
                    Icon(if (checked) Icons.Outlined.CheckBox else Icons.Outlined.CheckBoxOutlineBlank, null, Modifier.size(20.dp),
                        tint = if (checked) MaterialTheme.colorScheme.primary else CedarColors.secondaryText)
                    Spacer(Modifier.width(8.dp))
                    Text(
                        buildAnnotatedString {
                            ingredient.quantity?.let { withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append("$it ") } }
                            append(ingredient.item)
                        },
                        style = MaterialTheme.typography.bodyMedium.copy(textDecoration = if (checked) TextDecoration.LineThrough else null),
                        color = if (checked) CedarColors.secondaryText else Color.Unspecified,
                    )
                }
            }
        }
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            CardLabel(stringResource(R.string.steps))
            card.steps.forEachIndexed { index, step ->
                val isDone = index in done
                Row(verticalAlignment = Alignment.Top) {
                    Box(
                        Modifier.size(24.dp).clip(CircleShape)
                            .background(if (isDone) MaterialTheme.colorScheme.primary else Color.Transparent)
                            .border(1.5.dp, MaterialTheme.colorScheme.primary, CircleShape)
                            .clickable { if (isDone) done.remove(index) else done.add(index) },
                        contentAlignment = Alignment.Center,
                    ) {
                        Text("${index + 1}", style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.Bold,
                            color = if (isDone) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.primary)
                    }
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        if (step.title != null || step.duration != null) {
                            Text(listOfNotNull(step.title, step.duration).joinToString(" · "), style = MaterialTheme.typography.titleSmall)
                        }
                        Text(step.text, style = MaterialTheme.typography.bodyMedium, color = if (isDone) CedarColors.secondaryText else Color.Unspecified)
                    }
                }
            }
        }
        if (card.tips.isNotEmpty()) {
            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                CardLabel(stringResource(if (card.tips.size == 1) R.string.recipe_tip else R.string.recipe_tips))
                card.tips.forEach { tip ->
                    Row {
                        Icon(Icons.Outlined.Lightbulb, null, Modifier.size(16.dp), tint = CedarColors.yellow)
                        Spacer(Modifier.width(6.dp))
                        Text(tip, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
        }
    }
}

// plan_trip

private fun modeIcon(mode: TripCard.Mode): ImageVector = when (mode) {
    TripCard.Mode.TRANSIT -> Icons.Outlined.Tram
    TripCard.Mode.WALK -> Icons.AutoMirrored.Outlined.DirectionsWalk
    TripCard.Mode.DRIVE -> Icons.Outlined.DirectionsCar
    TripCard.Mode.RIDESHARE -> Icons.Outlined.LocalTaxi
}

private fun legIcon(kind: TripCard.LegKind): ImageVector = when (kind) {
    TripCard.LegKind.WALK -> Icons.AutoMirrored.Outlined.DirectionsWalk
    TripCard.LegKind.BIKE -> Icons.Outlined.DirectionsBike
    TripCard.LegKind.DRIVE -> Icons.Outlined.DirectionsCar
    TripCard.LegKind.BUS -> Icons.Outlined.DirectionsBus
    TripCard.LegKind.TRAM -> Icons.Outlined.Tram
    TripCard.LegKind.SUBWAY -> Icons.Outlined.DirectionsSubway
    TripCard.LegKind.FERRY -> Icons.Outlined.DirectionsBoat
    TripCard.LegKind.PLANE -> Icons.Outlined.Flight
    TripCard.LegKind.TRAIN -> Icons.Outlined.Train
    TripCard.LegKind.OTHER -> Icons.Outlined.Circle
}

/** A time of day in the trip's time zone when the planner gave one, else the phone's. */
private fun clock(instant: Instant, card: TripCard, weekday: Boolean = false): String {
    val zone = card.timeZone ?: ZoneId.systemDefault()
    return if (weekday) DateTimeFormatter.ofPattern("EEE ").withZone(zone).format(instant) + shortTime(instant, zone) else shortTime(instant, zone)
}

/** The facts under an option's name: its times, transfers, distance or price. */
private fun facts(context: Context, o: TripCard.Option, card: TripCard): String {
    val out = mutableListOf<String>()
    if (o.depart != null && o.arrive != null) out += "${clock(o.depart!!, card)} – ${clock(o.arrive!!, card)}"
    if (o.mode == TripCard.Mode.TRANSIT) {
        o.transfers?.let {
            out += if (it == 0) context.getString(R.string.trip_direct) else context.resources.getQuantityString(R.plurals.trip_transfers, it, it)
        }
        o.walkMinutes?.takeIf { it > 0 }?.let { out += context.getString(R.string.walk_minutes, it) }
    } else {
        TripStyle.distance(o.km, o.meters)?.let { out += it }
    }
    o.cost?.let { out += TripStyle.money(it) }
    return out.joinToString(" · ")
}

/** A trip: the recommendation, a map of the chosen option's route, the options, and the chosen one's steps. */
@Composable
fun TripCardView(card: TripCard) {
    val context = LocalContext.current
    var selected by rememberSaveable(card) { mutableStateOf((card.recommendedOption ?: card.options[0]).id) }
    val option = card.options.firstOrNull { it.id == selected } ?: card.options[0]
    val accent = MaterialTheme.colorScheme.primary
    val route = "${card.from.label} → ${card.to.label}"
    val whenText = card.arriveBy?.let { stringResource(R.string.arrive_by, clock(it, card, true)) }
        ?: card.departAt?.let { stringResource(R.string.leave_at, clock(it, card, true)) } ?: stringResource(R.string.leaving_now)

    // Each transit leg in its colour, walks dashed, a car route solid, or a dashed straight line without geometry.
    val lines = buildList {
        if (option.mode == TripCard.Mode.TRANSIT) {
            option.legs.filter { it.path.size > 1 }.forEach { leg ->
                val walk = leg.kind == TripCard.LegKind.WALK
                add(Route(leg.path, if (walk) Color.Gray else leg.color?.let(::hexColor) ?: accent, dashed = walk))
            }
        } else {
            // A rideshare drives the car's route.
            val path = if (option.path.size > 1) option.path else card.options.firstOrNull { it.mode == TripCard.Mode.DRIVE }?.path.orEmpty()
            if (path.size > 1) add(Route(path, if (option.mode == TripCard.Mode.WALK) Color.Gray else accent, dashed = option.mode == TripCard.Mode.WALK))
        }
        if (isEmpty()) add(Route(listOf(card.from.coordinate, card.to.coordinate), accent, dashed = true))
    }

    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Column {
            Text(card.title ?: route, style = MaterialTheme.typography.titleMedium)
            Text(if (card.title == null) whenText else "$route · $whenText", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
        card.recommendedOption?.let { rec ->
            Row(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(accent.copy(alpha = 0.1f)).padding(10.dp),
                verticalAlignment = Alignment.Top,
            ) {
                Icon(modeIcon(rec.mode), null, Modifier.size(20.dp), tint = accent)
                Spacer(Modifier.width(10.dp))
                Column {
                    Text(stringResource(tripTakeLabel(rec.mode), context.duration(rec.minutes)), style = MaterialTheme.typography.titleSmall)
                    card.reason?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                }
            }
        }
        androidx.compose.runtime.key(selected) {
            CardMap(
                card.title ?: route,
                listOf(Pin(card.from.coordinate, "A", card.from.label, CedarColors.green), Pin(card.to.coordinate, "B", card.to.label, CedarColors.red)),
                lines,
            )
        }
        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            card.options.forEach { o ->
                val isSelected = o.id == selected
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp))
                        .background(if (isSelected) accent.copy(alpha = 0.12f) else MaterialTheme.colorScheme.surfaceContainerHighest.copy(alpha = 0.5f))
                        .clickable { selected = o.id }.padding(10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(modeIcon(o.mode), null, Modifier.size(20.dp), tint = if (isSelected) accent else CedarColors.secondaryText)
                    Spacer(Modifier.width(10.dp))
                    Column(Modifier.weight(1f)) {
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            Text(stringResource(tripModeLabel(o.mode)), style = MaterialTheme.typography.titleSmall)
                            if (o.id == card.recommended) Text(stringResource(R.string.recommended), style = MaterialTheme.typography.labelSmall, color = accent)
                            if (o.id == card.fastest?.id && card.options.size > 1) {
                                Text(stringResource(R.string.fastest), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
                            }
                        }
                        val f = facts(context, o, card)
                        if (f.isNotEmpty()) Text(f, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                        o.note?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                    }
                    Text(context.duration(o.minutes), style = MaterialTheme.typography.titleSmall)
                }
            }
        }
        if (option.mode == TripCard.Mode.TRANSIT && option.legs.isNotEmpty()) TripSteps(card, option)
        if (card.unavailable.isNotEmpty() || card.attribution != null) {
            Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                card.unavailable.forEach {
                    Text(
                        stringResource(R.string.label_value, stringResource(tripModeLabel(it.mode)), it.reason ?: stringResource(R.string.not_available)),
                        style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText,
                    )
                }
                card.attribution?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText.copy(alpha = 0.7f)) }
            }
        }
    }
}

/** A transit option's steps: each walk and ride, with its line, times and stops. */
@Composable
private fun TripSteps(card: TripCard, option: TripCard.Option) {
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        CardLabel(stringResource(R.string.steps))
        option.legs.forEach { leg ->
            Row(verticalAlignment = Alignment.Top) {
                Icon(legIcon(leg.kind), null, Modifier.size(18.dp), tint = CedarColors.secondaryText)
                Spacer(Modifier.width(8.dp))
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        leg.route?.let { name ->
                            val color = leg.color?.let(::hexColor) ?: MaterialTheme.colorScheme.primary
                            Text(name, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold, color = inkOn(color),
                                modifier = Modifier.clip(RoundedCornerShape(4.dp)).background(color).padding(horizontal = 6.dp, vertical = 1.dp))
                        }
                        val what = if (leg.kind == TripCard.LegKind.WALK) {
                            leg.to?.let { stringResource(R.string.walk_to, it) } ?: stringResource(R.string.trip_walk)
                        } else {
                            val kind = stringResource(legKindLabel(leg.kind))
                            leg.headsign?.let { stringResource(R.string.leg_towards, kind, it) } ?: kind
                        }
                        Text(what, style = MaterialTheme.typography.bodyMedium)
                    }
                    val details = buildList {
                        if (leg.depart != null && leg.arrive != null) add("${clock(leg.depart!!, card)} – ${clock(leg.arrive!!, card)}")
                        if (leg.kind != TripCard.LegKind.WALK) {
                            leg.from?.let { add(context.getString(R.string.leg_from, it)) }
                            leg.stops?.let { add(context.resources.getQuantityString(R.plurals.trip_stops, it, it)) }
                        }
                        add(context.duration(leg.minutes))
                        if (leg.kind == TripCard.LegKind.WALK) TripStyle.distance(null, leg.meters)?.let { add(it) }
                        leg.agency?.let { add(it) }
                    }
                    Text(details.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                }
            }
        }
    }
}
