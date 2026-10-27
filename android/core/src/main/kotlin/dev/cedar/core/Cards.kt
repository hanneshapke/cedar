package dev.cedar.core

import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import java.net.URI
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.ZoneOffset
import kotlin.math.abs
import kotlin.math.ceil
import kotlin.math.ln
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt

/**
 * A tool result the app draws as a card: `show_map`, `show_recipe`, `plan_trip`, `compare_stocks`,
 * `search_products`, and the data of a `generate_chart` call. Parsed with the same rules as the web
 * UI's views and iOS/Cedar/Models/Cards.swift. Everything in a result came from the model or the
 * network, so text is clipped, numbers and coordinates are checked, and links are only kept if they
 * are https.
 */
sealed interface Card {
    data class Map(val card: MapCard) : Card
    data class Recipe(val card: RecipeCard) : Card
    data class Trip(val card: TripCard) : Card
    data class Stocks(val card: StocksCard) : Card
    data class Products(val card: ProductsCard) : Card
    data class Chart(val card: ChartCard) : Card

    companion object {
        /**
         * The card for a finished call, or `null` when it isn't a card tool or its result isn't usable.
         * A chart is drawn from the call's `input`; its output only says where the picture was saved.
         */
        fun parse(name: String, output: String, input: JsonElement = JsonNull): Card? {
            if (name !in ToolCall.CARD_TOOLS) return null
            if (name == "generate_chart") return ChartCard.parse(input)?.let(::Chart)
            val value = parseJson(output) ?: return null
            return when (name) {
                "show_map" -> MapCard.parse(value)?.let(::Map)
                "show_recipe" -> RecipeCard.parse(value)?.let(::Recipe)
                "plan_trip" -> TripCard.parse(value)?.let(::Trip)
                "compare_stocks" -> StocksCard.parse(value)?.let(::Stocks)
                "search_products" -> ProductsCard.parse(value)?.let(::Products)
                else -> null
            }
        }
    }
}

// Shared rules

object CardParse {
    /** Trimmed, non-empty text, at most `max` characters. */
    fun text(value: JsonElement?, max: Int = 200): String? =
        value.str?.trim()?.takeIf { it.isNotEmpty() }?.take(max)

    /** A number in `0..max`. */
    fun amount(value: JsonElement?, max: Double = 1e7): Double? = value.dbl?.takeIf { it >= 0 && it <= max }

    fun https(value: JsonElement?): String? {
        val s = value.str ?: return null
        if (!s.startsWith("https://") || s.any { it.isWhitespace() }) return null
        return try {
            URI(s)
            s
        } catch (_: Exception) {
            null
        }
    }

    fun time(value: JsonElement?): Instant? = value.str?.let(CedarDate::parse)

    fun isCoordinate(v: Double, limit: Double): Boolean = v.isFinite() && abs(v) <= limit

    /** `#rrggbb`, lowercased. */
    fun hexColor(value: JsonElement?): String? {
        val s = value.str ?: return null
        if (s.length != 7 || s[0] != '#' || !s.drop(1).all { it.isDigit() || it.lowercaseChar() in 'a'..'f' }) return null
        return s.lowercase()
    }

    fun fixed(v: Double, digits: Int): String = String.format(java.util.Locale.US, "%.${digits}f", v)
}

/** A point on the map. */
data class Coordinate(val latitude: Double, val longitude: Double)

/** A named point: a map pin, or where a trip starts or ends. */
data class Place(val label: String, val latitude: Double, val longitude: Double) {
    val coordinate: Coordinate get() = Coordinate(latitude, longitude)
    val coordinateText: String get() = "${CardParse.fixed(latitude, 4)}, ${CardParse.fixed(longitude, 4)}"

    companion object {
        fun parse(value: JsonElement?): Place? {
            val lat = value["latitude"].dbl?.takeIf { CardParse.isCoordinate(it, 90.0) } ?: return null
            val lon = value["longitude"].dbl?.takeIf { CardParse.isCoordinate(it, 180.0) } ?: return null
            val label = CardParse.text(value["label"]) ?: "${CardParse.fixed(lat, 4)}, ${CardParse.fixed(lon, 4)}"
            return Place(label, lat, lon)
        }
    }
}

// show_map

/** `{"title"?, "markers": [{"label", "latitude", "longitude"}], "not_found"?: [names]}` */
data class MapCard(val title: String?, val markers: List<Place>, val notFound: List<String>) {
    companion object {
        const val MAX_MARKERS = 25

        fun parse(value: JsonElement): MapCard? {
            val raw = value["markers"].arr ?: return null
            val markers = raw.take(MAX_MARKERS).mapNotNull(Place::parse)
            if (markers.isEmpty()) return null
            val notFound = value["not_found"].arr.orEmpty().mapNotNull { CardParse.text(it) }.take(MAX_MARKERS)
            return MapCard(CardParse.text(value["title"]), markers, notFound)
        }
    }
}

// show_recipe

/**
 * `{"title", "description"?, "servings"?, "prep_time"?, "cook_time"?, "total_time"?, "ingredients":
 * [{"quantity"?, "item"}], "steps": [{"title"?, "text", "duration"?}], "tips"?, "image"?: {"url",
 * "page_url", "title", "source"}}`
 */
data class RecipeCard(
    val title: String,
    val description: String?,
    val facts: List<Fact>,
    val ingredients: List<Ingredient>,
    val steps: List<Step>,
    val tips: List<String>,
    val photo: Photo?,
) {
    data class Fact(val label: String, val value: String)
    data class Ingredient(val quantity: String?, val item: String)
    data class Step(val title: String?, val text: String, val duration: String?)
    data class Photo(val url: String, val pageURL: String?, val title: String?, val source: String)

    companion object {
        fun parse(value: JsonElement): RecipeCard? {
            val title = CardParse.text(value["title"]) ?: return null
            val rawIngredients = value["ingredients"].arr ?: return null
            val rawSteps = value["steps"].arr ?: return null
            val ingredients = rawIngredients.take(60).mapNotNull { i ->
                CardParse.text(i["item"])?.let { Ingredient(CardParse.text(i["quantity"]), it) }
            }
            val steps = rawSteps.take(40).mapNotNull { s ->
                CardParse.text(s["text"], 2000)?.let { Step(CardParse.text(s["title"]), it, CardParse.text(s["duration"])) }
            }
            if (ingredients.isEmpty() || steps.isEmpty()) return null
            val tips = value["tips"].arr.orEmpty().mapNotNull { CardParse.text(it, 2000) }.take(10)
            val facts = listOf("Prep" to "prep_time", "Cook" to "cook_time", "Total" to "total_time", "Serves" to "servings")
                .mapNotNull { (label, key) -> CardParse.text(value[key])?.let { Fact(label, it) } }
            val image = value["image"]
            val photo = CardParse.https(image["url"])?.let { url ->
                Photo(url, CardParse.https(image["page_url"]), CardParse.text(image["title"]), CardParse.text(image["source"]) ?: "Wikimedia")
            }
            return RecipeCard(title, CardParse.text(value["description"], 2000), facts, ingredients, steps, tips, photo)
        }
    }
}

// plan_trip

/**
 * `{"title"?, "from", "to", "depart_at"? | "arrive_by"?, "time_zone"?, "recommendation": {"mode",
 * "option", "reason"}, "options": [...], "unavailable"?: [{"mode", "reason"}], "attribution"}`
 */
data class TripCard(
    val title: String?,
    val from: Place,
    val to: Place,
    val timeZone: ZoneId?,
    val departAt: Instant?,
    val arriveBy: Instant?,
    val options: List<Option>,
    val recommended: Int?,
    val reason: String?,
    val unavailable: List<Unavailable>,
    val attribution: String?,
) {
    enum class Mode(val raw: String, val label: String) {
        TRANSIT("transit", "Transit"), WALK("walk", "Walk"), DRIVE("drive", "Drive"), RIDESHARE("rideshare", "Rideshare");

        companion object {
            fun of(raw: String?): Mode? = entries.firstOrNull { it.raw == raw }
        }
    }

    /** A leg's mode, grouped by the icon and word it gets (MOTIS names, lowercased). */
    enum class LegKind(val label: String) {
        WALK("Walk"), BIKE("Bike"), DRIVE("Drive"), BUS("Bus"), TRAM("Tram"), SUBWAY("Subway"),
        FERRY("Ferry"), PLANE("Flight"), TRAIN("Train"), OTHER("Ride");

        companion object {
            fun motis(mode: String): LegKind = when (mode) {
                "walk" -> WALK
                "bike", "rental" -> BIKE
                "car", "car_parking", "odm" -> DRIVE
                "bus", "coach", "flex" -> BUS
                "tram", "cable_car", "funicular", "areal_lift" -> TRAM
                "subway", "metro" -> SUBWAY
                "ferry" -> FERRY
                "airplane" -> PLANE
                "rail", "suburban", "highspeed_rail", "long_distance", "night_rail", "regional_rail", "regional_fast_rail" -> TRAIN
                else -> OTHER
            }
        }
    }

    data class Leg(
        val kind: LegKind,
        val from: String?,
        val to: String?,
        val depart: Instant?,
        val arrive: Instant?,
        val minutes: Int,
        val route: String?,
        val headsign: String?,
        val agency: String?,
        /** `#rrggbb` */
        val color: String?,
        val stops: Int?,
        val meters: Double?,
        val path: List<Coordinate>,
    )

    data class Cost(val low: Double, val high: Double, val currency: String)

    data class Option(
        /** The option's index in the result, which the recommendation refers to. */
        val id: Int,
        val mode: Mode,
        val minutes: Int,
        val legs: List<Leg>,
        val cost: Cost?,
        val depart: Instant?,
        val arrive: Instant?,
        val transfers: Int?,
        val walkMinutes: Int?,
        val km: Double?,
        val meters: Double?,
        val note: String?,
        val path: List<Coordinate>,
    )

    data class Unavailable(val mode: Mode, val reason: String?)

    val recommendedOption: Option? get() = options.firstOrNull { it.id == recommended }
    val fastest: Option? get() = options.minByOrNull { it.minutes }

    companion object {
        fun parse(value: JsonElement): TripCard? {
            val raw = value["options"].arr ?: return null
            val from = Place.parse(value["from"]) ?: return null
            val to = Place.parse(value["to"]) ?: return null
            val options = raw.take(8).mapIndexedNotNull { index, o -> option(o, index) }
            if (options.isEmpty()) return null
            val rec = value["recommendation"]
            val recommended = rec["option"].int?.takeIf { i -> options.any { it.id == i } }
            val unavailable = value["unavailable"].arr.orEmpty().mapNotNull { u ->
                val mode = Mode.of(u["mode"].str) ?: return@mapNotNull null
                if (options.any { it.mode == mode }) null else Unavailable(mode, CardParse.text(u["reason"]))
            }.take(4)
            return TripCard(
                title = CardParse.text(value["title"]), from = from, to = to,
                timeZone = CardParse.text(value["time_zone"])?.let { runCatching { ZoneId.of(it) }.getOrNull() },
                departAt = CardParse.time(value["depart_at"]), arriveBy = CardParse.time(value["arrive_by"]),
                options = options, recommended = recommended,
                reason = if (recommended == null) null else CardParse.text(rec["reason"]),
                unavailable = unavailable, attribution = CardParse.text(value["attribution"]),
            )
        }

        private fun option(o: JsonElement, index: Int): Option? {
            val mode = Mode.of(o["mode"].str) ?: return null
            val minutes = CardParse.amount(o["duration_min"]) ?: return null
            val legs = if (mode == Mode.TRANSIT) o["legs"].arr.orEmpty().take(30).mapNotNull(::leg) else emptyList()
            val c = o["cost"]
            val low = CardParse.amount(c["low"])
            val high = CardParse.amount(c["high"])
            val cost = if (low != null && high != null) {
                Cost(low, high, CardParse.text(c["currency"])?.take(3)?.uppercase() ?: "USD")
            } else null
            return Option(
                id = index, mode = mode, minutes = minutes.roundToInt(), legs = legs, cost = cost,
                depart = CardParse.time(o["depart"]), arrive = CardParse.time(o["arrive"]),
                transfers = CardParse.amount(o["transfers"], 50.0)?.toInt(),
                walkMinutes = CardParse.amount(o["walk_min"])?.roundToInt(),
                km = CardParse.amount(o["distance_km"], 1e5), meters = CardParse.amount(o["distance_m"]),
                note = CardParse.text(o["note"]), path = Polyline.decode(o["path"].str),
            )
        }

        private fun leg(l: JsonElement): Leg? {
            if (l !is JsonObject) return null
            return Leg(
                kind = LegKind.motis(CardParse.text(l["mode"])?.lowercase() ?: ""),
                from = CardParse.text(l["from"]), to = CardParse.text(l["to"]),
                depart = CardParse.time(l["depart"]), arrive = CardParse.time(l["arrive"]),
                minutes = (CardParse.amount(l["duration_min"]) ?: 0.0).roundToInt(),
                route = CardParse.text(l["route"])?.take(24),
                headsign = CardParse.text(l["headsign"]), agency = CardParse.text(l["agency"]),
                color = CardParse.hexColor(l["color"]),
                stops = CardParse.amount(l["stops"], 1000.0)?.toInt(),
                meters = CardParse.amount(l["distance_m"]),
                path = Polyline.decode(l["path"].str),
            )
        }
    }
}

/** Google's encoded polyline format, precision 5. */
object Polyline {
    /** The points in `encoded`, or none if it doesn't decode cleanly. */
    fun decode(encoded: String?): List<Coordinate> {
        if (encoded == null) return emptyList()
        val bytes = encoded.toByteArray(Charsets.UTF_8)
        if (bytes.size > 20_000) return emptyList()
        var i = 0
        fun next(): Int? {
            var result = 0
            var shift = 0
            while (true) {
                if (i >= bytes.size || shift > 30) return null
                val b = (bytes[i].toInt() and 0xff) - 63
                i += 1
                if (b !in 0..63) return null
                result = result or ((b and 0x1f) shl shift)
                shift += 5
                if (b < 0x20) break
            }
            return if (result and 1 != 0) (result shr 1).inv() else result shr 1
        }
        val points = mutableListOf<Coordinate>()
        var lat = 0
        var lon = 0
        while (i < bytes.size && points.size < 2000) {
            val dlat = next() ?: return emptyList()
            val dlon = next() ?: return emptyList()
            lat += dlat
            lon += dlon
            val point = Coordinate(lat / 1e5, lon / 1e5)
            if (!CardParse.isCoordinate(point.latitude, 90.0) || !CardParse.isCoordinate(point.longitude, 180.0)) return emptyList()
            points += point
        }
        return points
    }
}

// compare_stocks

/**
 * `{"range", "benchmark"?, "benchmark_series"?, "as_of", "symbols": [...], "news": [...],
 * "news_note"?, "unavailable"?, "disclaimer"}`
 */
data class StocksCard(
    /** The period the chart starts on, a key of `RANGES`. */
    val range: String,
    val asOf: Instant?,
    val stocks: List<Stock>,
    val benchmark: Benchmark?,
    val news: List<News>,
    val newsNote: String?,
    val unavailable: List<Unavailable>,
    val disclaimer: String?,
) {
    /** Daily closes, oldest first, at least two. */
    data class Series(val dates: List<Instant>, val closes: List<Double>) {
        companion object {
            fun parse(chart: JsonElement?): Series? {
                val dates = chart["dates"].arr ?: return null
                val closes = chart["close"].arr ?: return null
                val outDates = mutableListOf<Instant>()
                val outCloses = mutableListOf<Double>()
                for ((d, c) in dates.zip(closes).take(2000)) {
                    val date = d.str?.let(::day) ?: continue
                    val close = c.dbl?.takeIf { it > 0 } ?: continue
                    if (outDates.isNotEmpty() && date <= outDates.last()) continue
                    outDates += date
                    outCloses += close
                }
                return if (outDates.size >= 2) Series(outDates, outCloses) else null
            }
        }
    }

    enum class Call(val label: String) {
        BUY("Buy"), HOLD("Hold"), SELL("Sell");

        companion object {
            fun of(raw: String?): Call? = entries.firstOrNull { it.name.lowercase() == raw }
        }
    }

    data class Factor(val name: String, val headline: String?, val detail: String, /** -2..2 */ val score: Int)

    data class Rating(
        val call: Call,
        val lean: String?,
        val score: Int,
        val maxScore: Int,
        /** low, medium, or high */
        val confidence: String,
        val summary: String?,
        val factors: List<Factor>,
        val watch: List<String>,
    ) {
        companion object {
            fun parse(r: JsonElement?): Rating? {
                val call = Call.of(r["call"].str) ?: return null
                val factors = r["factors"].arr.orEmpty().take(12).mapNotNull { f ->
                    val name = CardParse.text(f["name"], 80) ?: return@mapNotNull null
                    val detail = CardParse.text(f["detail"], 1200) ?: return@mapNotNull null
                    val score = Math.round(f["score"].dbl ?: 0.0).toInt()
                    Factor(name, CardParse.text(f["headline"], 80), detail, score.coerceIn(-2, 2))
                }
                val maxScore = max(1, Math.round(r["max_score"].dbl ?: 9.0).toInt())
                val score = Math.round(r["score"].dbl ?: 0.0).toInt()
                val confidence = r["confidence"].str ?: ""
                return Rating(
                    call = call, lean = CardParse.text(r["lean"], 40),
                    score = score.coerceIn(-maxScore, maxScore), maxScore = maxScore,
                    confidence = if (confidence in listOf("low", "medium", "high")) confidence else "low",
                    summary = CardParse.text(r["summary"], 1200), factors = factors,
                    watch = r["watch"].arr.orEmpty().mapNotNull { CardParse.text(it, 1200) }.take(6),
                )
            }
        }
    }

    data class Indicators(
        val sma50: Double?,
        val sma200: Double?,
        val rsi: Double?,
        val volatility: Double?,
        val drawdown: Double?,
        val beta: Double?,
    )

    data class Stock(
        val symbol: String,
        val name: String,
        val currency: String?,
        val price: Double,
        val change: Double?,
        val changePct: Double?,
        val dayLow: Double?,
        val dayHigh: Double?,
        val low52: Double?,
        val high52: Double?,
        /** One value per `RETURN_PERIODS` entry. */
        val returns: List<Double?>,
        val indicators: Indicators,
        val rating: Rating?,
        val series: Series,
    )

    data class Benchmark(val symbol: String, val name: String, /** `null` when the benchmark is one of the symbols. */ val series: Series?)

    data class News(val title: String, val url: String, val publisher: String?, val published: Instant?, val symbols: List<String>)

    data class Unavailable(val symbol: String, val reason: String?)

    companion object {
        val RETURN_PERIODS = listOf("1w" to "1W", "1m" to "1M", "3m" to "3M", "ytd" to "YTD", "6m" to "6M", "1y" to "1Y")

        /** The chart periods, in days. */
        val RANGES = listOf("1M" to 30, "3M" to 91, "6M" to 182, "1Y" to 365, "2Y" to 730, "5Y" to 1826)
        private val RANGE_KEYS = mapOf("1mo" to "1M", "3mo" to "3M", "6mo" to "6M", "1y" to "1Y", "2y" to "2Y", "5y" to "5Y")

        fun parse(value: JsonElement): StocksCard? {
            val raw = value["symbols"].arr ?: return null
            val stocks = raw.take(6).mapNotNull { s ->
                val symbol = CardParse.text(s["symbol"], 20) ?: return@mapNotNull null
                val price = s["price"].dbl ?: return@mapNotNull null
                val series = Series.parse(s["chart"]) ?: return@mapNotNull null
                val ind = s["indicators"]
                Stock(
                    symbol = symbol, name = CardParse.text(s["name"], 120) ?: symbol,
                    currency = CardParse.text(s["currency"], 8), price = price,
                    change = s["change"].dbl, changePct = s["change_pct"].dbl,
                    dayLow = s["day_low"].dbl, dayHigh = s["day_high"].dbl,
                    low52 = s["fifty_two_week_low"].dbl, high52 = s["fifty_two_week_high"].dbl,
                    returns = RETURN_PERIODS.map { (key, _) -> s["returns"][key].dbl },
                    indicators = Indicators(
                        sma50 = ind["sma50"].dbl, sma200 = ind["sma200"].dbl, rsi = ind["rsi14"].dbl,
                        volatility = ind["volatility_pct"].dbl, drawdown = ind["max_drawdown_pct"].dbl, beta = ind["beta"].dbl,
                    ),
                    rating = Rating.parse(s["rating"]), series = series,
                )
            }
            if (stocks.isEmpty()) return null

            val b = value["benchmark"]
            val benchmark = CardParse.text(b["symbol"], 20)?.let { symbol ->
                val series = if (b["in_symbols"].bool == true) null else Series.parse(value["benchmark_series"])
                Benchmark(symbol, CardParse.text(b["name"], 120) ?: symbol, series)
            }
            val symbols = stocks.map { it.symbol }.toSet()
            val news = value["news"].arr.orEmpty().take(24).mapNotNull { n ->
                val url = CardParse.https(n["url"]) ?: return@mapNotNull null
                val title = CardParse.text(n["title"], 300) ?: return@mapNotNull null
                News(title, url, CardParse.text(n["publisher"], 80), CardParse.time(n["published"]),
                    n["symbols"].arr.orEmpty().mapNotNull { it.str }.filter { it in symbols })
            }
            val unavailable = value["unavailable"].arr.orEmpty().mapNotNull { u ->
                CardParse.text(u["symbol"], 20)?.let { Unavailable(it, CardParse.text(u["reason"])) }
            }
            return StocksCard(
                range = value["range"].str?.let { RANGE_KEYS[it] } ?: "1Y",
                asOf = CardParse.time(value["as_of"]), stocks = stocks, benchmark = benchmark, news = news,
                newsNote = CardParse.text(value["news_note"], 400), unavailable = unavailable,
                disclaimer = CardParse.text(value["disclaimer"], 600),
            )
        }

        /** A `yyyy-MM-dd` day, at midnight UTC. */
        fun day(s: String): Instant? {
            if (s.length != 10) return null
            return try {
                LocalDate.parse(s).atStartOfDay().toInstant(ZoneOffset.UTC)
            } catch (_: Exception) {
                null
            }
        }
    }
}

// search_products

/** The products a `search_products` call found and weighed, as product-view.js reads them. */
data class ProductsCard(
    /** What was searched for; `null` when the result doesn't say (the app says "Products"). */
    val query: String?,
    /** Where; `null` when the result doesn't say (the app says "the store"). */
    val store: String?,
    val currency: String,
    val total: Int,
    /** How many pages were read and products found, for a web search. */
    val pages: Int?,
    val productsFound: Int?,
    val asOf: Instant?,
    /** How the options are ordered: a key of [SORTS] (the app words it), or `null`. */
    val sort: String?,
    /** The model's recommendation, when it weighed the options. */
    val summary: String?,
    val note: String?,
    val options: List<Product>,
    /** The rest of the search, which comparable options may point at. */
    val others: List<Product>,
    /** The badges each option has. */
    val badges: kotlin.collections.Map<String, List<String>>,
    val sources: kotlin.collections.Map<Int, Source>,
    val disclaimer: String?,
) {
    /** What a product's reviews amount to, as cedar reads them. */
    enum class Verdict(val raw: String, val label: String) {
        EXCELLENT("excellent", "Excellent"), GOOD("good", "Good"), MIXED("mixed", "Mixed"),
        POOR("poor", "Poor"), FEW("few", "Few reviews"), UNREVIEWED("none", "No rating");

        companion object {
            fun of(raw: String?): Verdict? = entries.firstOrNull { it.raw == raw }
        }
    }

    data class Reviews(
        /** 0..5 */
        val average: Double?,
        val count: Int,
        /** The average shrunk toward a typical rating by how few reviews there are, 0..5. */
        val adjusted: Double?,
        val verdict: Verdict,
        /** low, medium, or high */
        val confidence: String,
        val summary: String?,
    ) {
        companion object {
            fun parse(r: JsonElement?): Reviews {
                val count = (CardParse.amount(r["count"], 1e9) ?: 0.0).toInt()
                val fallback = if (count > 0) Verdict.FEW else Verdict.UNREVIEWED
                val confidence = r["confidence"].str ?: ""
                return Reviews(
                    average = if (count > 0) CardParse.amount(r["average"], 5.0) else null, count = count,
                    adjusted = if (count > 0) CardParse.amount(r["adjusted"], 5.0) else null,
                    verdict = Verdict.of(r["verdict"].str) ?: fallback,
                    confidence = if (confidence in listOf("low", "medium", "high")) confidence else "low",
                    summary = CardParse.text(r["summary"], 600),
                )
            }
        }
    }

    data class Spec(val name: String, val value: String)

    /** A page the picks rest on, numbered as the model saw it. */
    data class Source(val title: String, val url: String?, val host: String?)

    data class Product(
        /** The store's sku, or the pick's id (`P1`). */
        val id: String,
        val name: String,
        val brand: String?,
        val model: String?,
        val color: String?,
        val condition: String?,
        /** `null` when no page stated one. */
        val price: Double?,
        /** A three-letter code; the card's when the product has none. */
        val currency: String?,
        /** The price comes from a store page or catalog, not from review or search text. */
        val priceChecked: Boolean,
        /** Only when it is above `price`. */
        val regularPrice: Double?,
        val savingsPct: Double?,
        val url: String?,
        /** Who sells it, e.g. "nike.com" or "Best Buy". */
        val store: String?,
        /** https only, the main photo first. */
        val images: List<String>,
        val description: String?,
        val features: List<String>,
        val specs: List<Spec>,
        val category: String?,
        /** What the store said about stock; `null` when it didn't. */
        val online: Boolean?,
        val inStore: Boolean?,
        val freeShipping: Boolean?,
        val reviews: Reviews,
        /** The badges the result gives it (`Top pick`, `Best value`, ...), if it names them. */
        val badges: List<String>?,
        val why: String?,
        val pros: List<String>,
        val cons: List<String>,
        val bestFor: String?,
        /** Numbers of the card's sources behind it. */
        val sources: List<Int>,
        /** Skus of other products in the card. */
        val comparable: List<String>,
    )

    fun product(sku: String): Product? = options.firstOrNull { it.id == sku } ?: others.firstOrNull { it.id == sku }

    /** The currency `product`'s price is in. */
    fun currencyOf(product: Product): String = product.currency ?: currency

    companion object {
        const val MAX_OPTIONS = 8
        /** The orders a search can have, in English words. */
        val SORTS = mapOf(
            "best_selling" to "best selling first", "rating" to "highest rated first",
            "price_low" to "cheapest first", "price_high" to "priciest first",
        )
        /** The picks a search names, with their English badges (the app translates these). */
        val PICKS = listOf("top_pick" to "Top pick", "best_value" to "Best value", "best_rated" to "Best rated", "most_popular" to "Most popular")

        /** A sku given as a number or a string. */
        fun sku(value: JsonElement?): String? {
            val n = value.dbl
            if (n != null && n >= 0 && n < 1e15) return Math.round(n).toString()
            return CardParse.text(value, 40)
        }

        fun parse(value: JsonElement): ProductsCard? {
            val raw = value["options"].arr ?: return null
            var options = raw.take(MAX_OPTIONS).mapNotNull(::parseProduct)
            if (options.isEmpty()) return null
            val shown = options.map { it.id }.toSet()
            val seen = shown.toMutableSet()
            val others = value["others"].arr.orEmpty().take(40).mapNotNull(::parseProduct).filter { seen.add(it.id) }
            val known = seen.toSet()
            options = options.map { o -> o.copy(comparable = o.comparable.filter { it != o.id && it in known }.take(4)) }
            val badges = linkedMapOf<String, List<String>>()
            for ((key, label) in PICKS) {
                val pick = sku(value["picks"][key])
                if (pick != null && pick in shown) badges[pick] = badges[pick].orEmpty() + label
            }
            // An option that names its own badges (the tool's web search does) keeps exactly those.
            for (option in options) option.badges?.let { badges[option.id] = it }
            val sources = linkedMapOf<Int, Source>()
            for (s in value["sources"].arr.orEmpty().take(40)) {
                val n = CardParse.amount(s["n"], 1000.0)?.toInt() ?: continue
                val title = CardParse.text(s["title"]) ?: CardParse.text(s["host"], 80) ?: continue
                sources[n] = Source(title, CardParse.https(s["url"]), CardParse.text(s["host"], 80))
            }
            val considered = value["considered"]
            return ProductsCard(
                query = CardParse.text(value["query"], 160),
                store = CardParse.text(value["store"], 60),
                currency = currencyCode(value["currency"]) ?: "USD",
                total = max(options.size + others.size, (CardParse.amount(value["total"], 1e9) ?: 0.0).toInt()),
                pages = CardParse.amount(considered["pages"], 1e6)?.toInt(),
                productsFound = CardParse.amount(considered["products"], 1e6)?.toInt(),
                asOf = CardParse.time(value["as_of"]),
                sort = value["sort"].str?.takeIf { it in SORTS },
                summary = CardParse.text(value["summary"], 800),
                note = CardParse.text(value["note"], 600),
                options = options, others = others, badges = badges, sources = sources,
                disclaimer = CardParse.text(value["disclaimer"], 600),
            )
        }

        /** A three-letter currency code, upper-cased. */
        private fun currencyCode(value: JsonElement?): String? {
            val c = CardParse.text(value, 4) ?: return null
            if (c.length != 3 || !c.all { it.isLetter() }) return null
            return c.uppercase()
        }

        private fun strings(value: JsonElement?, max: Int, count: Int): List<String> =
            value.arr.orEmpty().mapNotNull { CardParse.text(it, max) }.take(count)

        private fun parseProduct(p: JsonElement): Product? {
            val id = sku(p["sku"]) ?: return null
            val name = CardParse.text(p["name"]) ?: return null
            val price = CardParse.amount(p["price"])
            val rawImages = p["images"].arr ?: listOf(p["image"] ?: JsonNull)
            val images = mutableListOf<String>()
            for (url in rawImages.mapNotNull(CardParse::https)) if (url !in images && images.size < 6) images += url
            val regular = CardParse.amount(p["regular_price"])?.takeIf { r -> price != null && r > price }
            val availability = p["availability"]
            return Product(
                id = id, name = name,
                brand = CardParse.text(p["brand"], 80), model = CardParse.text(p["model"], 80),
                color = CardParse.text(p["color"], 60), condition = CardParse.text(p["condition"], 40),
                price = price, currency = currencyCode(p["currency"]),
                priceChecked = p["price_checked"].bool != false,
                regularPrice = regular,
                savingsPct = if (regular == null) null else CardParse.amount(p["savings_pct"], 100.0),
                url = CardParse.https(p["url"]), store = CardParse.text(p["store"], 80), images = images,
                description = CardParse.text(p["description"], 600),
                features = p["features"].arr.orEmpty().mapNotNull { CardParse.text(it) }.take(6),
                specs = p["specs"].arr.orEmpty().mapNotNull { s ->
                    val n = CardParse.text(s["name"], 80) ?: return@mapNotNull null
                    val v = CardParse.text(s["value"]) ?: return@mapNotNull null
                    Spec(n, v)
                }.take(8),
                category = CardParse.text(p["category"], 80),
                online = availability["online"].bool,
                inStore = availability["in_store"].bool,
                freeShipping = availability["free_shipping"].bool,
                reviews = Reviews.parse(p["reviews"]),
                badges = p["badges"].arr?.mapNotNull { CardParse.text(it, 40) },
                why = CardParse.text(p["why"], 600),
                pros = strings(p["pros"], 120, 4),
                cons = strings(p["cons"], 120, 4),
                bestFor = CardParse.text(p["best_for"], 160),
                sources = p["sources"].arr.orEmpty().mapNotNull { CardParse.amount(it, 1000.0)?.toInt() }.take(6),
                comparable = p["comparable"].arr.orEmpty().mapNotNull { sku(it) },
            )
        }
    }
}

// generate_chart

/**
 * A `generate_chart` call's input: `{"path", "kind", "labels"?, "series": [{"name"?, "values"}],
 * "title"?, "x_label"?, "y_label"?, "stacked"?, "bins"?, "log_scale"?}`. Long series are thinned to
 * what a card can show; the saved picture has every point.
 */
data class ChartCard(
    val kind: Kind,
    val path: String?,
    val title: String?,
    val xLabel: String?,
    val yLabel: String?,
    val labels: List<Label>,
    val series: List<Series>,
    val stacked: Boolean,
    val bins: Int?,
    val log: Boolean,
    /** A shaded range on the value axis, such as a target range. */
    val band: Band?,
    val thinned: Boolean,
) {
    enum class Kind(val raw: String, val title: String) {
        LINE("line", "Line chart"), BAR("bar", "Bar chart"), BARH("barh", "Bar chart"), AREA("area", "Area chart"),
        SCATTER("scatter", "Scatter plot"), PIE("pie", "Pie chart"), HISTOGRAM("histogram", "Histogram");

        companion object {
            fun of(raw: String?): Kind? = entries.firstOrNull { it.raw == raw }
        }
    }

    /** An x-axis category or value. */
    sealed interface Label {
        val text: String
        val number: Double?

        data class Text(val value: String) : Label {
            override val text get() = value
            override val number: Double? get() = null
        }

        data class Number(val value: Double) : Label {
            override val text get() = format(value)
            override val number: Double get() = value
        }
    }

    data class Series(
        val name: String,
        /** Whether the model named it, rather than it being "Series 2". */
        val named: Boolean,
        /** One per label; `null` is a gap. For a histogram, the raw samples. */
        val values: List<Double?>,
    )

    data class Band(val from: Double, val to: Double, val label: String?)

    /** Whether the x values are numbers (a scatter, or a line or area over numbers). */
    val numericX: Boolean
        get() = kind == Kind.SCATTER || ((kind == Kind.LINE || kind == Kind.AREA) && labels.all { it.number != null })

    /** Histogram bins over every series' samples: the edges, and the counts per series. */
    val histogram: Pair<List<Double>, List<List<Int>>>?
        get() {
            val samples = series.map { s -> s.values.filterNotNull() }
            val all = samples.flatten()
            var lo = all.minOrNull() ?: return null
            var hi = all.maxOrNull() ?: return null
            if (lo == hi) {
                lo -= 0.5
                hi += 0.5
            }
            val n = bins ?: max(5, min(40, ceil(ln(all.size.toDouble()) / ln(2.0) + 1).toInt()))
            val width = (hi - lo) / n
            val edges = (0..n).map { lo + it * width }
            val counts = samples.map { values ->
                val c = IntArray(n)
                for (v in values) c[min(n - 1, ((v - lo) / width).toInt())] += 1
                c.toList()
            }
            return edges to counts
        }

    companion object {
        const val MAX_SERIES = 20
        const val MAX_VALUES = 100_000

        /** The categorical palette's size; later series are drawn in grey. */
        const val SLOTS = 8
        const val MAX_OUTPUT_CHARTS = 6

        fun parse(value: JsonElement?): ChartCard? {
            val kind = Kind.of(value["kind"].str) ?: return null
            val rawSeries = value["series"].arr?.takeIf { it.isNotEmpty() } ?: return null
            var series = mutableListOf<Series>()
            for ((i, s) in rawSeries.take(MAX_SERIES).withIndex()) {
                val raw = s["values"].arr?.takeIf { it.isNotEmpty() } ?: return null
                val values = mutableListOf<Double?>()
                for (v in raw.take(MAX_VALUES)) {
                    if (v is JsonNull) {
                        values += null
                        continue
                    }
                    values += v.dbl ?: return null
                }
                val name = CardParse.text(s["name"], 80)
                series += Series(name ?: "Series ${i + 1}", name != null, values)
            }
            var labels = mutableListOf<Label>()
            if (kind != Kind.HISTOGRAM) {
                val raw = value["labels"].arr?.takeIf { it.isNotEmpty() } ?: return null
                for (l in raw.take(MAX_VALUES)) {
                    val n = l.dbl
                    val s = l.str
                    labels += when {
                        n != null -> Label.Number(n)
                        s != null -> Label.Text(s.take(120))
                        else -> return null
                    }
                }
                if (!series.all { it.values.size == labels.size }) return null
                if (kind == Kind.SCATTER && !labels.all { it.number != null }) return null
            }
            if (kind == Kind.PIE) {
                if (series.size != 1 || !series[0].values.all { (it ?: -1.0) >= 0 } || series[0].values.none { (it ?: 0.0) > 0 }) return null
            }

            var thinned = false
            if (kind != Kind.HISTOGRAM && kind != Kind.PIE) {
                val limit = when (kind) {
                    Kind.BAR -> 300
                    Kind.BARH -> 60
                    Kind.SCATTER -> 2000
                    else -> 800
                }
                if (labels.size > limit) {
                    thinned = true
                    val step = labels.size.toDouble() / limit
                    val keep = (0 until limit).map { (it * step).toInt() }.toMutableList()
                    if (keep.last() != labels.size - 1) keep += labels.size - 1
                    labels = keep.map { labels[it] }.toMutableList()
                    series = series.map { s -> s.copy(values = keep.map { s.values[it] }) }.toMutableList()
                }
            }
            val bins = value["bins"].dbl?.takeIf { it in 1.0..500.0 }?.toInt()
            val a = value["band"]["from"].dbl
            val b = value["band"]["to"].dbl
            val band = if (kind != Kind.PIE && a != null && b != null && a != b) {
                Band(min(a, b), max(a, b), CardParse.text(value["band"]["label"], 40))
            } else null
            return ChartCard(
                kind = kind,
                path = CardParse.text(value["path"], 300),
                title = CardParse.text(value["title"]),
                xLabel = CardParse.text(value["x_label"], 120),
                yLabel = CardParse.text(value["y_label"], 120),
                labels = labels,
                series = series,
                stacked = value["stacked"].bool == true && kind in setOf(Kind.BAR, Kind.BARH, Kind.AREA, Kind.HISTOGRAM),
                bins = bins,
                log = value["log_scale"].bool == true && kind != Kind.PIE && kind != Kind.HISTOGRAM,
                band = band,
                thinned = thinned,
            )
        }

        data class Found(val charts: List<ChartCard>, val text: String)

        /**
         * The charts a tool printed as `[chart] {spec}` lines (generate_chart's input, without a path),
         * and its output without them; the same rule as the web UI's `cedarChart.fromOutput`.
         */
        fun inOutput(output: String): Found {
            if (!output.contains("[chart] ")) return Found(emptyList(), output)
            val charts = mutableListOf<ChartCard>()
            val kept = mutableListOf<String>()
            for (line in output.split("\n")) {
                if (charts.size < MAX_OUTPUT_CHARTS && line.startsWith("[chart] {")) {
                    val value = parseJson(line.removePrefix("[chart] "))
                    if (value is JsonObject) {
                        val chart = parse(JsonObject(value - "path"))
                        if (chart != null) {
                            charts += chart
                            continue
                        }
                    }
                }
                kept += line
            }
            if (charts.isEmpty()) return Found(emptyList(), output)
            return Found(charts, kept.joinToString("\n").trim())
        }

        /** A number as it reads in a tooltip or table. */
        fun format(v: Double): String {
            val a = abs(v)
            val digits = if (a >= 1000) 0 else if (a >= 100) 1 else if (a >= 1) 2 else if (a == 0.0) 0 else 4
            val f = java.text.NumberFormat.getNumberInstance()
            f.minimumFractionDigits = 0
            f.maximumFractionDigits = digits
            return f.format(v)
        }
    }
}
