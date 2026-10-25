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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.automirrored.outlined.OpenInNew
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarHalf
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Remove
import androidx.compose.material.icons.outlined.StarOutline
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import coil.compose.AsyncImage
import dev.cedar.cedar.ui.CedarColors
import dev.cedar.cedar.ui.cardFrame
import dev.cedar.cedar.ui.openUrl
import dev.cedar.cedar.ui.shortDateTime
import dev.cedar.core.ProductsCard
import dev.cedar.core.number

private fun money(value: Double, currency: String): String {
    val symbol = mapOf("USD" to "$", "EUR" to "€", "GBP" to "£", "JPY" to "¥", "CAD" to "CA$", "AUD" to "A$")[currency]
    val n = number(value, 2, 2)
    return if (symbol != null) "$symbol$n" else "$n $currency"
}

private fun verdictColor(v: ProductsCard.Verdict): Color = when (v) {
    ProductsCard.Verdict.EXCELLENT, ProductsCard.Verdict.GOOD -> CedarColors.green
    ProductsCard.Verdict.MIXED -> CedarColors.orange
    ProductsCard.Verdict.POOR -> CedarColors.red
    else -> Color.Gray
}

/**
 * A `search_products` card: the model's recommendation; a tile per option (photo, badges, price, stars
 * and the verdict on the reviews), which also picks the product shown below; the selected product with
 * its photos, stock, a link to the store, why it fits, pros and cons, reviews, features, specs and
 * sources; and the comparable options, which can be viewed in turn.
 */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun ProductsCardView(card: ProductsCard) {
    val context = LocalContext.current
    var selected by remember(card) { mutableStateOf(card.options[0].id) }
    // The option a comparable product was opened from, for the way back.
    var origin by remember(card) { mutableStateOf<String?>(null) }
    var photo by remember(card) { mutableStateOf(0) }
    val product = card.product(selected) ?: card.options[0]
    fun select(sku: String, from: String?) {
        selected = sku
        origin = from
        photo = 0
    }

    Column(Modifier.cardFrame(), verticalArrangement = Arrangement.spacedBy(14.dp)) {
        Column {
            Text(card.query.replaceFirstChar { it.uppercase() }, style = MaterialTheme.typography.titleMedium)
            val meta = buildList {
                add("${card.options.size} of ${card.total} from ${card.store}")
                if (card.pages != null && card.productsFound != null) add("${card.productsFound} products on ${card.pages} pages")
                card.sort?.let { add(it) }
            }
            Text(meta.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
        card.summary?.let {
            Column(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(10.dp)).background(MaterialTheme.colorScheme.primary.copy(alpha = 0.08f)).padding(10.dp),
                verticalArrangement = Arrangement.spacedBy(4.dp),
            ) {
                CardLabel("The recommendation")
                Text(it, style = MaterialTheme.typography.bodyMedium)
            }
        }
        card.note?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }

        // A tile per option.
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            card.options.forEach { p ->
                val isSelected = p.id == selected
                val accent = MaterialTheme.colorScheme.primary
                Column(
                    Modifier.width(150.dp).clip(RoundedCornerShape(10.dp))
                        .background(if (isSelected) accent.copy(alpha = 0.1f) else MaterialTheme.colorScheme.surfaceContainerHighest.copy(alpha = 0.5f))
                        .border(1.dp, if (isSelected) accent.copy(alpha = 0.6f) else Color.Transparent, RoundedCornerShape(10.dp))
                        .clickable { select(p.id, null) }.padding(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                ) {
                    ProductPhoto(p.images.firstOrNull(), p.name, 90)
                    card.badges[p.id]?.firstOrNull()?.let { PickBadge(it) }
                    Text(p.name, style = MaterialTheme.typography.labelLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    p.price?.let { Text(money(it, card.currencyOf(p)), style = MaterialTheme.typography.titleSmall) }
                    RatingLine(p.reviews)
                }
            }
        }
        HorizontalDivider()

        // The selected product.
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            origin?.let { from ->
                card.product(from)?.let { o ->
                    Row(Modifier.clickable { select(o.id, null) }, verticalAlignment = Alignment.CenterVertically) {
                        Icon(Icons.AutoMirrored.Outlined.ArrowBack, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
                        Spacer(Modifier.width(4.dp))
                        Text("Back to ${o.name}", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary, maxLines = 1)
                    }
                }
            }
            if (product.images.isNotEmpty()) {
                ProductPhoto(product.images.getOrNull(photo) ?: product.images[0], product.name, 220)
                if (product.images.size > 1) {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        product.images.indices.forEach { i ->
                            Box(Modifier.size(8.dp).clip(RoundedCornerShape(4.dp))
                                .background(if (i == photo) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant)
                                .clickable { photo = i })
                        }
                    }
                }
            }
            card.badges[product.id]?.takeIf { it.isNotEmpty() }?.let { badges ->
                FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) { badges.forEach { PickBadge(it) } }
            }
            Text(product.name, style = MaterialTheme.typography.titleMedium)
            listOfNotNull(product.brand, product.model, product.color, product.condition).takeIf { it.isNotEmpty() }?.let {
                Text(it.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
            PriceLine(product, card.currencyOf(product))
            val stock = buildList {
                product.online?.let { add(if (it) "Available online" else "Not available online") }
                product.inStore?.let { add(if (it) "In stores" else "Not in stores") }
                if (product.freeShipping == true) add("Free shipping")
            }
            if (stock.isNotEmpty()) Text(stock.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            product.url?.let { url ->
                Row(Modifier.clickable { openUrl(context, url) }, verticalAlignment = Alignment.CenterVertically) {
                    Icon(Icons.AutoMirrored.Outlined.OpenInNew, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.primary)
                    Spacer(Modifier.width(4.dp))
                    Text("View at ${product.store ?: card.store}", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                }
            }
            if (product.why != null || product.bestFor != null) {
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    CardLabel("Why it fits")
                    product.why?.let { Text(it, style = MaterialTheme.typography.bodyMedium) }
                    product.bestFor?.let {
                        Text(buildAnnotatedString {
                            withStyle(SpanStyle(fontWeight = FontWeight.SemiBold)) { append("Best for: ") }
                            append(it)
                        }, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
            if (product.pros.isNotEmpty() || product.cons.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    product.pros.forEach { Bullet(Icons.Outlined.Add, CedarColors.green, it) }
                    product.cons.forEach { Bullet(Icons.Outlined.Remove, CedarColors.red, it) }
                }
            }
            ReviewsPanel(product.reviews)
            product.description?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
            if (product.features.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    CardLabel("Key features")
                    product.features.forEach { Text("• $it", style = MaterialTheme.typography.bodySmall) }
                }
            }
            if (product.specs.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    CardLabel("Specs")
                    product.specs.forEach { spec ->
                        Row {
                            Text(spec.name, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                            Text(spec.value, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
            val sources = product.sources.mapNotNull { n -> card.sources[n]?.let { n to it } }
            if (sources.isNotEmpty()) {
                Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                    CardLabel("Sources")
                    sources.forEach { (n, source) ->
                        Text(
                            "[$n] ${source.title}${source.host?.let { " · $it" } ?: ""}",
                            style = MaterialTheme.typography.bodySmall,
                            color = if (source.url != null) MaterialTheme.colorScheme.primary else Color.Unspecified,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = source.url?.let { url -> Modifier.clickable { openUrl(context, url) } } ?: Modifier,
                        )
                    }
                }
            }
        }

        // Comparable options.
        val comparable = product.comparable.mapNotNull(card::product)
        if (comparable.isNotEmpty()) {
            Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                CardLabel("Comparable options")
                comparable.forEach { c ->
                    Row(
                        Modifier.fillMaxWidth().clip(RoundedCornerShape(8.dp)).clickable { select(c.id, if (card.options.any { it.id == product.id }) product.id else origin) }.padding(4.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Box(Modifier.size(44.dp)) { ProductPhoto(c.images.firstOrNull(), c.name, 44) }
                        Spacer(Modifier.width(8.dp))
                        Column(Modifier.weight(1f)) {
                            Text(c.name, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            RatingLine(c.reviews)
                        }
                        c.price?.let { Text(money(it, card.currencyOf(c)), style = MaterialTheme.typography.labelLarge) }
                    }
                }
            }
        }

        Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
            card.asOf?.let { Text("As of ${shortDateTime(it)}", style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText) }
            card.disclaimer?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText.copy(alpha = 0.7f)) }
        }
    }
}

@Composable
private fun ProductPhoto(url: String?, name: String, height: Int) {
    Box(
        Modifier.fillMaxWidth().height(height.dp).clip(RoundedCornerShape(8.dp)).background(Color.White),
        contentAlignment = Alignment.Center,
    ) {
        if (url != null) {
            AsyncImage(model = url, contentDescription = name, modifier = Modifier.fillMaxWidth().height(height.dp).padding(4.dp), contentScale = ContentScale.Fit)
        }
    }
}

@Composable
private fun PriceLine(product: ProductsCard.Product, currency: String) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(product.price?.let { money(it, currency) } ?: "Price at the store", style = MaterialTheme.typography.titleLarge)
            product.regularPrice?.let {
                Text(money(it, currency), style = MaterialTheme.typography.bodyMedium.copy(textDecoration = TextDecoration.LineThrough), color = CedarColors.secondaryText)
            }
            product.savingsPct?.let {
                Text("Save ${Math.round(it)}%", style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.Bold, color = CedarColors.green,
                    modifier = Modifier.clip(RoundedCornerShape(50)).background(CedarColors.green.copy(alpha = 0.15f)).padding(horizontal = 6.dp, vertical = 2.dp))
            }
        }
        if (product.price != null && !product.priceChecked) {
            Text("From review or search text, not checked on a store page", style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        }
    }
}

@Composable
private fun Stars(rating: Double, size: Int = 12) {
    Row {
        (1..5).forEach { i ->
            val icon = when {
                rating >= i -> Icons.Filled.Star
                rating >= i - 0.5 -> Icons.Filled.StarHalf
                else -> Icons.Outlined.StarOutline
            }
            Icon(icon, null, Modifier.size(size.dp), tint = CedarColors.yellow)
        }
    }
}

@Composable
private fun RatingLine(reviews: ProductsCard.Reviews) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        val average = reviews.average
        if (average != null) {
            Stars(average)
            Text("(${java.text.NumberFormat.getIntegerInstance().format(reviews.count)})", style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        } else {
            Text("No reviews", style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        }
    }
}

@Composable
private fun ReviewsPanel(reviews: ProductsCard.Reviews) {
    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        CardLabel("Reviews")
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val color = verdictColor(reviews.verdict)
            Text(reviews.verdict.label, style = MaterialTheme.typography.labelMedium, fontWeight = FontWeight.Bold, color = color,
                modifier = Modifier.clip(RoundedCornerShape(50)).background(color.copy(alpha = 0.15f)).padding(horizontal = 8.dp, vertical = 2.dp))
            val average = reviews.average
            if (average != null) {
                Stars(average, 14)
                Text("${"%.1f".format(average)} from ${java.text.NumberFormat.getIntegerInstance().format(reviews.count)} review${if (reviews.count == 1) "" else "s"}",
                    style = MaterialTheme.typography.bodySmall)
            } else {
                Text("No customer rating found", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
        }
        reviews.adjusted?.let {
            Text("Adjusted for the number of reviews: ${"%.1f".format(it)} / 5 · ${reviews.confidence} confidence", style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        }
        reviews.summary?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
    }
}

@Composable
private fun PickBadge(text: String) {
    Text(text, style = MaterialTheme.typography.labelSmall, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onPrimary,
        modifier = Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.primary).padding(horizontal = 6.dp, vertical = 2.dp))
}

@Composable
private fun Bullet(icon: androidx.compose.ui.graphics.vector.ImageVector, color: Color, text: String) {
    Row(verticalAlignment = Alignment.Top) {
        Icon(icon, null, Modifier.size(14.dp), tint = color)
        Spacer(Modifier.width(6.dp))
        Text(text, style = MaterialTheme.typography.bodySmall)
    }
}
