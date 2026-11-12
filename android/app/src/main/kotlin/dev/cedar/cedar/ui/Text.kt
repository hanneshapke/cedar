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
import dev.cedar.core.Agent
import dev.cedar.core.ChartCard
import dev.cedar.core.ChatException
import dev.cedar.core.DeviceStart
import dev.cedar.core.FlightsCard
import dev.cedar.core.OutgoingAttachment
import dev.cedar.core.Predicate
import dev.cedar.core.ProductsCard
import dev.cedar.core.RecipeCard
import dev.cedar.core.Span
import dev.cedar.core.ServerSettings
import dev.cedar.core.StocksCard
import dev.cedar.core.Trigger
import dev.cedar.core.TriggerGroup
import dev.cedar.core.TriggerKind
import dev.cedar.core.TriggerProblem
import dev.cedar.core.TripCard
import dev.cedar.core.WatchSource
import java.time.Instant
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.util.Locale
import java.util.TimeZone

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

/** A `HH:MM` as the user's locale writes times; anything else as it is. */
fun clockText(time: String): String =
    runCatching { LocalTime.parse(time).format(DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT)) }.getOrDefault(time)

/** "Every 15 minutes", "Every 2 hours", "Every day". */
fun Context.everyText(minutes: Int): String {
    val (n, unit) = Span.split(minutes)
    // "Every hour" has words of its own; a plural's `one` can't leave the number out in every language.
    if (n == 1) {
        return getString(
            when (unit) {
                Span.MINUTES -> R.string.every_one_minute
                Span.HOURS -> R.string.every_one_hour
                Span.DAYS -> R.string.every_one_day
            },
        )
    }
    val id = when (unit) {
        Span.MINUTES -> R.plurals.every_minutes
        Span.HOURS -> R.plurals.every_hours
        Span.DAYS -> R.plurals.every_days
    }
    return resources.getQuantityString(id, n, n)
}

/** A condition's test in a few words: "changes", "below 40". */
fun Context.predicateText(predicate: Predicate): String = when (predicate.op) {
    "changes" -> getString(R.string.op_changes_text)
    "gt" -> getString(R.string.op_gt_text, predicate.valueText)
    "gte" -> getString(R.string.op_gte_text, predicate.valueText)
    "lt" -> getString(R.string.op_lt_text, predicate.valueText)
    "lte" -> getString(R.string.op_lte_text, predicate.valueText)
    "equals" -> getString(R.string.op_equals_text, predicate.valueText)
    "contains" -> getString(R.string.op_contains_text, predicate.valueText)
    // A newer server's test, in its own word.
    else -> listOf(predicate.op, predicate.valueText).filter { it.isNotEmpty() }.joinToString(" ")
}

/** A test's name, for the form's picker. */
@StringRes
fun opLabel(op: String): Int? = when (op) {
    "changes" -> R.string.op_changes
    "gt" -> R.string.op_gt
    "gte" -> R.string.op_gte
    "lt" -> R.string.op_lt
    "lte" -> R.string.op_lte
    "equals" -> R.string.op_equals
    "contains" -> R.string.op_contains
    else -> null
}

/**
 * What starts an agent, in one line: "Weekdays at 07:30", "Every 2 hours", "example.com · price ·
 * below 40", with the trigger's time zone when it isn't this device's.
 */
fun Context.describe(trigger: Trigger): String {
    val text = when (trigger) {
        is Trigger.Time -> {
            val time = clockText(trigger.time)
            when (trigger.frequency) {
                "daily" -> getString(R.string.schedule_daily, time)
                "weekdays" -> getString(R.string.schedule_weekdays, time)
                "weekly" -> trigger.weekday?.let { WEEKLY.getOrNull(it) }?.let { getString(it, time) } ?: getString(R.string.schedule_weekly, time)
                else -> getString(R.string.schedule_other, trigger.frequency.replaceFirstChar { it.uppercase() }, time)
            }
        }
        is Trigger.Once -> {
            val at = runCatching { LocalDateTime.parse(trigger.at).format(DateTimeFormatter.ofLocalizedDateTime(FormatStyle.MEDIUM, FormatStyle.SHORT)) }
                .getOrDefault(trigger.at)
            getString(R.string.trigger_once, at)
        }
        is Trigger.Interval -> everyText(trigger.everyMinutes)
        is Trigger.Monthly -> getString(R.string.trigger_monthly, trigger.day, clockText(trigger.time))
        is Trigger.Condition -> {
            val source = when (val s = trigger.source) {
                // The host says enough; the whole address is on the agent's page.
                is WatchSource.Url -> runCatching { java.net.URI(s.url).host }.getOrNull()?.takeIf { it.isNotEmpty() } ?: s.url
                is WatchSource.Tool -> s.name
                is WatchSource.Records -> getString(R.string.trigger_records, s.set)
            }
            listOfNotNull(source, trigger.extract?.takeIf { it.isNotEmpty() }, predicateText(trigger.predicate)).joinToString(" · ")
        }
        Trigger.Webhook -> getString(R.string.trigger_webhook)
        is Trigger.Email -> if (trigger.query.isEmpty()) getString(R.string.trigger_email_any) else getString(R.string.trigger_email, trigger.query)
        is Trigger.Calendar -> {
            val before = trigger.minutesBefore?.let { if (it == 0) getString(R.string.trigger_calendar_start) else getString(R.string.trigger_calendar_before, duration(it)) }
            val new = if (trigger.newEvents) getString(R.string.trigger_calendar_new) else null
            val filter = trigger.filter?.takeIf { it.isNotEmpty() }?.let { getString(R.string.trigger_calendar_filter, it) }
            listOfNotNull(before, new, filter).joinToString(" · ")
        }
        is Trigger.Records -> getString(if (trigger.on == Trigger.Records.CHANGED) R.string.trigger_records_changed else R.string.trigger_records_added, trigger.set)
        // The folder's ID says nothing; it is on the agent's page.
        is Trigger.Drive -> getString(R.string.trigger_drive)
        Trigger.Device -> getString(R.string.trigger_device)
        is Trigger.Unknown -> if (trigger.kind.isEmpty()) getString(R.string.trigger_unknown) else getString(R.string.trigger_unknown_kind, trigger.kind)
    }
    val zone = trigger.timezone?.takeIf { it != TimeZone.getDefault().id }
    return zone?.let { "$text ($it)" } ?: text
}

/** The heading agents with such a trigger are listed under. */
@StringRes
fun triggerGroupLabel(group: TriggerGroup): Int = when (group) {
    TriggerGroup.SCHEDULE -> R.string.agents_group_schedule
    TriggerGroup.CONDITION -> R.string.agents_group_condition
    TriggerGroup.WEBHOOK -> R.string.agents_group_webhook
    TriggerGroup.MAIL_CALENDAR -> R.string.agents_group_mail_calendar
    TriggerGroup.OTHER -> R.string.agents_group_other
}

/** A kind of trigger, for the form's picker. */
@StringRes
fun triggerKindLabel(kind: TriggerKind): Int = when (kind) {
    TriggerKind.TIME -> R.string.trigger_kind_time
    TriggerKind.ONCE -> R.string.trigger_kind_once
    TriggerKind.INTERVAL -> R.string.trigger_kind_interval
    TriggerKind.MONTHLY -> R.string.trigger_kind_monthly
    TriggerKind.CONDITION -> R.string.trigger_kind_condition
    TriggerKind.WEBHOOK -> R.string.trigger_kind_webhook
    TriggerKind.EMAIL -> R.string.trigger_kind_email
    TriggerKind.CALENDAR -> R.string.trigger_kind_calendar
    TriggerKind.RECORDS -> R.string.trigger_kind_records
    TriggerKind.DRIVE -> R.string.trigger_kind_drive
    TriggerKind.DEVICE -> R.string.trigger_kind_device
}

/** What an agent is doing, for its row: "Next in 2 hours", "Watching · checked 3 minutes ago", "Paused". */
fun Context.describe(status: Agent.Status): String = when (status) {
    Agent.Status.Paused -> getString(R.string.paused)
    Agent.Status.Done -> getString(R.string.outcome_done)
    is Agent.Status.CheckFailing -> getString(R.string.agent_check_failing)
    is Agent.Status.NextRun -> status.at?.let { getString(R.string.next_run, relative(it)) } ?: getString(R.string.agent_on)
    is Agent.Status.Watching -> status.lastCheckedAt?.let { getString(R.string.agent_watching_checked, relative(it)) } ?: getString(R.string.agent_watching)
    Agent.Status.WaitingForCalls -> getString(R.string.agent_waiting_for_calls)
    Agent.Status.CallWaiting -> getString(R.string.agent_call_waiting)
    Agent.Status.WaitingForDevice -> getString(R.string.agent_waiting_for_device)
    Agent.Status.Active -> getString(R.string.agent_on)
}

/** Why a trigger being edited can't be saved yet. */
fun Context.describe(problem: TriggerProblem): String = when (problem) {
    TriggerProblem.Time -> getString(R.string.trigger_problem_time)
    TriggerProblem.Date -> getString(R.string.trigger_problem_date)
    TriggerProblem.InThePast -> getString(R.string.trigger_problem_past)
    TriggerProblem.Day -> getString(R.string.trigger_problem_day)
    is TriggerProblem.Every -> getString(R.string.trigger_problem_every, duration(problem.min), duration(problem.max))
    TriggerProblem.Url -> getString(R.string.trigger_problem_url)
    TriggerProblem.Tool -> getString(R.string.trigger_problem_tool)
    TriggerProblem.ToolInput -> getString(R.string.trigger_problem_tool_input)
    TriggerProblem.RecordSet -> getString(R.string.trigger_problem_record_set)
    TriggerProblem.Folder -> getString(R.string.trigger_problem_folder)
    TriggerProblem.Number -> getString(R.string.trigger_problem_number)
    TriggerProblem.Value -> getString(R.string.trigger_problem_value)
    TriggerProblem.CalendarNeedsOne -> getString(R.string.trigger_problem_calendar)
    is TriggerProblem.MinutesBefore -> getString(R.string.trigger_problem_minutes_before, duration(problem.max))
}

/** Why an agent the device asked for didn't start: the request is not queued, so it says when to ask again. */
fun Context.describeHeld(held: String?): String = when (held) {
    DeviceStart.BUSY -> getString(R.string.device_held_busy)
    DeviceStart.TOO_SOON -> getString(R.string.device_held_too_soon)
    DeviceStart.DAILY_LIMIT -> getString(R.string.device_held_daily_limit)
    DeviceStart.CEDAR_BUSY -> getString(R.string.device_held_cedar_busy)
    // A newer server's reason, in its own word.
    else -> if (held.isNullOrBlank()) getString(R.string.device_held) else getString(R.string.device_held_other, held)
}

/** A notification method's name, as the server's Settings calls it; one this app doesn't know as it is. */
fun Context.notifyMethodLabel(method: String): String = when (method) {
    "none" -> getString(R.string.notify_method_none)
    "desktop" -> getString(R.string.notify_method_desktop)
    "browser" -> getString(R.string.notify_method_browser)
    "ios" -> getString(R.string.notify_method_ios)
    "android" -> getString(R.string.notify_method_android)
    "pushover" -> getString(R.string.notify_method_pushover)
    "email" -> getString(R.string.notify_method_email)
    else -> method
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
