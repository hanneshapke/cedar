package dev.cedar.cedar.ui

import android.content.Context
import android.text.format.DateFormat
import android.text.format.Formatter
import androidx.annotation.PluralsRes
import androidx.annotation.StringRes
import androidx.compose.runtime.Composable
import androidx.compose.ui.ExperimentalComposeUiApi
import androidx.compose.ui.res.pluralStringResource
import dev.cedar.cedar.R
import dev.cedar.core.ChartCard
import dev.cedar.core.ChatException
import dev.cedar.core.FlightsCard
import dev.cedar.core.OutgoingAttachment
import dev.cedar.core.ProductsCard
import dev.cedar.core.RecipeCard
import dev.cedar.core.Schedule
import dev.cedar.core.ServerSettings
import dev.cedar.core.StocksCard
import dev.cedar.core.TripCard
import java.time.Instant
import java.time.LocalTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale

// The app's words for what core describes in English or by key, and formatting in the user's locale
// (the default Locale, which Language sets to it).

/** A plural in the user's language, with `args` (usually the count) filled in. */
@OptIn(ExperimentalComposeUiApi::class)
@Composable
fun plural(@PluralsRes id: Int, count: Int, vararg args: Any): String = pluralStringResource(id, count, *args)

/** What went wrong, for the user: the app's words for its own errors, the server's for the API's. */
fun Context.describe(error: Throwable): String = when (error) {
    ChatException.NotConfigured -> getString(R.string.error_not_configured)
    ChatException.InvalidURL -> getString(R.string.error_invalid_url)
    ChatException.Unauthorized -> getString(R.string.error_unauthorized)
    is ChatException.Unreachable -> getString(
        R.string.error_unreachable,
        when (val hint = error.hint) {
            ChatException.Unreachable.Hint.HttpsOnly -> getString(R.string.error_https_only)
            is ChatException.Unreachable.Hint.PlainHttpPort -> getString(R.string.error_plain_http_port, hint.error, hint.port)
            null -> error.detail
        },
    )
    is ChatException.BadResponse -> getString(R.string.error_bad_response, error.detail)
    // The server's message is for developers and stays as it is; the app words only its own fallback.
    is ChatException.Api -> if (error.code == "http_${error.status}") getString(R.string.error_http_status, error.status) else error.text
    else -> error.message ?: error.toString()
}

fun Context.urlProblem(problem: ServerSettings.UrlProblem): String = when (problem) {
    ServerSettings.UrlProblem.NOT_CONFIGURED -> getString(R.string.error_not_configured)
    ServerSettings.UrlProblem.INVALID -> getString(R.string.error_invalid_url)
    ServerSettings.UrlProblem.NOT_HTTPS -> getString(R.string.error_https_only)
}

/** A file size as the user's locale writes it ("20 MB", "20 Mo"). */
fun Context.fileSize(bytes: Long): String = Formatter.formatShortFileSize(this, bytes)

fun Context.attachmentProblem(problem: OutgoingAttachment.Problem): String = when (problem) {
    is OutgoingAttachment.Problem.ImagesNotAccepted -> getString(R.string.attach_images_not_accepted, problem.name)
    is OutgoingAttachment.Problem.Empty -> getString(R.string.attach_empty, problem.name)
    is OutgoingAttachment.Problem.TooLarge -> getString(R.string.attach_too_large, problem.name, fileSize(problem.limit))
    is OutgoingAttachment.Problem.TooMany -> resources.getQuantityString(R.plurals.attach_too_many, problem.limit, problem.limit)
    is OutgoingAttachment.Problem.TooLargeTogether -> getString(R.string.attach_too_large_total, fileSize(problem.limit))
}

@StringRes
fun planModeLabel(mode: String): Int? = when (mode) {
    "" -> R.string.plan_mode_server
    "tool" -> R.string.plan_mode_tool
    "auto" -> R.string.plan_mode_auto
    "on" -> R.string.plan_mode_on
    "off" -> R.string.plan_mode_off
    else -> null
}

private val WEEKLY = listOf(
    R.string.schedule_mondays, R.string.schedule_tuesdays, R.string.schedule_wednesdays, R.string.schedule_thursdays,
    R.string.schedule_fridays, R.string.schedule_saturdays, R.string.schedule_sundays,
)

/** "Weekdays at 07:30", with the schedule's time zone when it isn't this device's. */
fun Context.cadence(schedule: Schedule): String {
    val time = runCatching { LocalTime.parse(schedule.timeOfDay).format(DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT)) }
        .getOrDefault(schedule.timeOfDay)
    val text = when (schedule.frequency) {
        "daily" -> getString(R.string.schedule_daily, time)
        "weekdays" -> getString(R.string.schedule_weekdays, time)
        "weekly" -> schedule.weekday?.let { WEEKLY.getOrNull(it) }?.let { getString(it, time) } ?: getString(R.string.schedule_weekly, time)
        else -> getString(R.string.schedule_other, schedule.frequency.replaceFirstChar { it.uppercase() }, time)
    }
    return schedule.zoneLabel()?.let { "$text ($it)" } ?: text
}

/** A date and time in the user's locale, from a skeleton such as `EEEdMMMHHmm` (fields, not order). */
fun skeletonDate(instant: Instant, skeleton: String, zone: ZoneId = ZoneId.systemDefault()): String {
    val locale = Locale.getDefault()
    return DateTimeFormatter.ofPattern(DateFormat.getBestDateTimePattern(locale, skeleton), locale).withZone(zone).format(instant)
}

/** "1 h 15 min", in the user's language. */
fun Context.duration(minutes: Int): String {
    if (minutes < 60) return getString(R.string.duration_minutes, minutes)
    val h = minutes / 60
    val m = minutes % 60
    return if (m == 0) getString(R.string.duration_hours, h) else getString(R.string.duration_hours_minutes, h, m)
}

@StringRes
fun tripModeLabel(mode: TripCard.Mode): Int = when (mode) {
    TripCard.Mode.TRANSIT -> R.string.trip_transit
    TripCard.Mode.WALK -> R.string.trip_walk
    TripCard.Mode.DRIVE -> R.string.trip_drive
    TripCard.Mode.RIDESHARE -> R.string.trip_rideshare
}

@StringRes
fun tripTakeLabel(mode: TripCard.Mode): Int = when (mode) {
    TripCard.Mode.TRANSIT -> R.string.trip_take_transit
    TripCard.Mode.WALK -> R.string.trip_take_walk
    TripCard.Mode.DRIVE -> R.string.trip_take_drive
    TripCard.Mode.RIDESHARE -> R.string.trip_take_rideshare
}

@StringRes
fun legKindLabel(kind: TripCard.LegKind): Int = when (kind) {
    TripCard.LegKind.WALK -> R.string.trip_walk
    TripCard.LegKind.BIKE -> R.string.leg_bike
    TripCard.LegKind.DRIVE -> R.string.trip_drive
    TripCard.LegKind.BUS -> R.string.leg_bus
    TripCard.LegKind.TRAM -> R.string.leg_tram
    TripCard.LegKind.SUBWAY -> R.string.leg_subway
    TripCard.LegKind.FERRY -> R.string.leg_ferry
    TripCard.LegKind.PLANE -> R.string.leg_flight
    TripCard.LegKind.TRAIN -> R.string.leg_train
    TripCard.LegKind.OTHER -> R.string.leg_ride
}

/** A recipe fact's label, as core names them. */
@StringRes
fun factLabel(fact: RecipeCard.Fact): Int? = when (fact.label) {
    "Prep" -> R.string.recipe_prep
    "Cook" -> R.string.recipe_cook
    "Total" -> R.string.recipe_total
    "Serves" -> R.string.recipe_serves
    else -> null
}

@StringRes
fun callLabel(call: StocksCard.Call): Int = when (call) {
    StocksCard.Call.BUY -> R.string.call_buy
    StocksCard.Call.HOLD -> R.string.call_hold
    StocksCard.Call.SELL -> R.string.call_sell
}

/** "Low confidence", or lowercased for the middle of a line. */
@StringRes
fun confidenceLabel(confidence: String, lower: Boolean = false): Int = when (confidence) {
    "high" -> if (lower) R.string.confidence_high_lower else R.string.confidence_high
    "medium" -> if (lower) R.string.confidence_medium_lower else R.string.confidence_medium
    else -> if (lower) R.string.confidence_low_lower else R.string.confidence_low
}

/** A chart or return period (`1W`, `YTD`, `5Y`). */
@StringRes
fun periodLabel(key: String): Int? = when (key) {
    "1W" -> R.string.period_1w
    "1M" -> R.string.period_1m
    "3M" -> R.string.period_3m
    "6M" -> R.string.period_6m
    "YTD" -> R.string.period_ytd
    "1Y" -> R.string.period_1y
    "2Y" -> R.string.period_2y
    "5Y" -> R.string.period_5y
    else -> null
}

@StringRes
fun verdictLabel(verdict: ProductsCard.Verdict): Int = when (verdict) {
    ProductsCard.Verdict.EXCELLENT -> R.string.verdict_excellent
    ProductsCard.Verdict.GOOD -> R.string.verdict_good
    ProductsCard.Verdict.MIXED -> R.string.verdict_mixed
    ProductsCard.Verdict.POOR -> R.string.verdict_poor
    ProductsCard.Verdict.FEW -> R.string.verdict_few
    ProductsCard.Verdict.UNREVIEWED -> R.string.verdict_none
}

/** A pick's badge, when it is one of the picks core names; the model's own badges stay as they are. */
@StringRes
fun badgeLabel(badge: String): Int? = when (ProductsCard.PICKS.firstOrNull { it.second == badge }?.first) {
    "top_pick" -> R.string.badge_top_pick
    "best_value" -> R.string.badge_best_value
    "best_rated" -> R.string.badge_best_rated
    "most_popular" -> R.string.badge_most_popular
    else -> null
}

@StringRes
fun sortLabel(sort: String): Int? = when (sort) {
    "best_selling" -> R.string.sort_best_selling
    "rating" -> R.string.sort_rating
    "price_low" -> R.string.sort_price_low
    "price_high" -> R.string.sort_price_high
    else -> null
}

@StringRes
fun chartKindLabel(kind: ChartCard.Kind): Int = when (kind) {
    ChartCard.Kind.LINE -> R.string.chart_line
    ChartCard.Kind.BAR, ChartCard.Kind.BARH -> R.string.chart_bar
    ChartCard.Kind.AREA -> R.string.chart_area
    ChartCard.Kind.SCATTER -> R.string.chart_scatter
    ChartCard.Kind.PIE -> R.string.chart_pie
    ChartCard.Kind.HISTOGRAM -> R.string.chart_histogram
}

@StringRes
fun cabinLabel(cabin: FlightsCard.Cabin): Int = when (cabin) {
    FlightsCard.Cabin.ECONOMY -> R.string.cabin_economy
    FlightsCard.Cabin.PREMIUM_ECONOMY -> R.string.cabin_premium_economy
    FlightsCard.Cabin.BUSINESS -> R.string.cabin_business
    FlightsCard.Cabin.FIRST -> R.string.cabin_first
}

/** "Prices are low for this route", for the levels Google words; others are shown as given. */
@StringRes
fun priceLevelLabel(level: String): Int? = when (level) {
    "low" -> R.string.flights_prices_low
    "typical" -> R.string.flights_prices_typical
    "high" -> R.string.flights_prices_high
    else -> null
}
