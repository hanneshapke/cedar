package dev.cedar.cedar.ui

import android.text.format.DateFormat
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowLeft
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.outlined.ArrowDropDown
import androidx.compose.material.icons.outlined.SmartToy
import androidx.compose.material.icons.outlined.Today
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TimePicker
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.material3.rememberTimePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.Agent
import dev.cedar.core.AskDraft
import dev.cedar.core.AskFrequency
import dev.cedar.core.AskProblem
import dev.cedar.core.AskWatch
import dev.cedar.core.AskWhen
import dev.cedar.core.OutboxItem
import dev.cedar.core.TimelineTurn
import dev.cedar.core.TurnState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.time.DayOfWeek
import java.time.Instant
import java.time.LocalDate
import java.time.LocalDateTime
import java.time.LocalTime
import java.time.ZoneId
import java.time.ZoneOffset
import java.time.format.DateTimeFormatter
import java.time.format.FormatStyle
import java.time.format.TextStyle
import java.util.Locale

/** How often the river is read again while its tab is in view. */
private const val REFRESH_MILLIS = 15_000L

/**
 * Today: the day as a river, as the web's /today/ page. The box to ask comes first, then what is
 * now (responses still going, waiting on an OK or a go-ahead, or asking a question, and what
 * agents' runs held for approval), what was earlier today, newest first, and what is ahead (the
 * agents due next and the ones that keep watch). A risky call is allowed or denied and a held
 * call approved right here; a question, a plan and a response still going open their chat; a
 * finished ask opens its outcome ([OutcomeScreen]); an agent opens its page.
 */
@Composable
fun TodayRows(app: AppModel, onAction: (suspend () -> Unit) -> Unit) {
    var asking by rememberSaveable { mutableStateOf(false) }
    // The tab's opening reads the river (see ConversationListScreen); this keeps it fresh.
    LaunchedEffect(Unit) {
        while (true) {
            delay(REFRESH_MILLIS)
            app.refreshTimeline()
        }
    }
    val day = app.timeline
    // The day shown: another one has no now, and nothing to ask from.
    val shownDay = app.timelineDay?.let { runCatching { LocalDate.parse(it) }.getOrNull() } ?: LocalDate.now()
    val isToday = app.timelineDay == null
    // What waits on the person, turns and held calls alike, in the order it came.
    val now: List<Pair<Instant, Any>> = buildList<Pair<Instant, Any>> {
        day?.now?.forEach { add(it.createdAt to it) }
        day?.outbox?.forEach { add((it.createdAt ?: Instant.EPOCH) to it) }
    }.sortedBy { it.first }
    TodayTheme {
    LazyColumn(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background), contentPadding = PaddingValues(bottom = 24.dp)) {
        if (!app.timelineUnsupported) {
            item(key = "day") {
                DayBar(
                    shownDay, isToday,
                    onShift = { days -> onAction { app.shiftDay(days) } },
                    onToday = { onAction { app.showDay(null) } },
                )
            }
        }
        app.timelineError?.let { error -> item(key = "error") { ErrorLine(error) } }
        if (isToday && day != null && day.needsYou > 0) item(key = "needs") { NeedsYouLine(day.needsYou) }
        // Now is one block: where to ask, and everything going or waiting on the person.
        if (isToday) item(key = "now") {
            TodayNowBlock(Modifier.padding(horizontal = 8.dp, vertical = 6.dp)) {
                NowHeader()
                AskBar(enabled = app.settings.isConfigured && !app.timelineUnsupported) { asking = true }
                now.forEach { (_, entry) ->
                    key(if (entry is TimelineTurn) "turn-${entry.turnId}" else "held-${(entry as OutboxItem).id}") {
                        when (entry) {
                            is TimelineTurn -> NowCard(app, entry, onAction)
                            is OutboxItem -> HeldEntry(app, entry)
                        }
                    }
                }
            }
        }
        when {
            app.timelineUnsupported -> item(key = "unsupported") {
                EmptyState(
                    Icons.Outlined.Today, stringResource(R.string.today_unsupported_title), stringResource(R.string.today_unsupported_text),
                    Modifier.padding(top = 56.dp),
                )
            }
            day == null -> if (app.timelineError == null) item(key = "loading") { Loading() }
            else -> {
                item(key = "earlier-header") { TodayHeader(stringResource(R.string.today_earlier)) }
                if (day.earlier.isEmpty()) {
                    item(key = "earlier-empty") { QuietLine(stringResource(if (isToday) R.string.today_earlier_empty else R.string.today_day_empty)) }
                }
                items(day.earlier.asReversed(), key = { "earlier-${it.turnId}" }) { turn ->
                    EarlierRow(turn) { app.showingOutcome = turn.turnId }
                }
                val ahead = day.ahead + day.watching
                if (ahead.isNotEmpty()) {
                    item(key = "ahead-header") { TodayHeader(stringResource(R.string.today_ahead)) }
                    items(ahead, key = { "ahead-${it.id}" }) { agent -> AheadRow(agent, shownDay) { app.openAgent(agent.id) } }
                }
            }
        }
    }
    if (asking) AskSheet(app, onDismiss = { asking = false })
    }
}

/** The line that opens the block of what is now: the time, the word, and a rule, in the lime. */
@Composable
private fun NowHeader() {
    val lime = MaterialTheme.colorScheme.primary
    Row(
        Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(shortTime(Instant.now()), style = MaterialTheme.typography.labelMedium, fontFamily = FontFamily.Monospace, color = lime)
        Text(
            stringResource(R.string.today_now).uppercase(Locale.getDefault()),
            style = MaterialTheme.typography.labelMedium,
            fontWeight = FontWeight.Bold,
            letterSpacing = 1.5.sp,
            color = lime,
        )
        Box(Modifier.weight(1f).height(2.dp).background(lime))
    }
}

/** A button for what needs the person: the amber, filled. */
@Composable
private fun needColors() = ButtonDefaults.buttonColors(
    containerColor = MaterialTheme.colorScheme.secondary,
    contentColor = MaterialTheme.colorScheme.onSecondary,
)

/** The day the river shows, with the days around it a tap away, and the way back to today. */
@Composable
private fun DayBar(day: LocalDate, isToday: Boolean, onShift: (Long) -> Unit, onToday: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton(onClick = { onShift(-1) }) {
            Icon(Icons.AutoMirrored.Outlined.KeyboardArrowLeft, stringResource(R.string.today_prev_day))
        }
        Text(
            day.format(DateTimeFormatter.ofLocalizedDate(FormatStyle.FULL)),
            Modifier.weight(1f),
            style = MaterialTheme.typography.titleSmall,
            fontWeight = FontWeight.SemiBold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        if (!isToday) TextButton(onClick = onToday) { Text(stringResource(R.string.today_back_to_today)) }
        IconButton(onClick = { onShift(1) }) {
            Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, stringResource(R.string.today_next_day))
        }
    }
}

@Composable
private fun TodayHeader(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.labelMedium,
        fontWeight = FontWeight.SemiBold,
        color = CedarColors.secondaryText,
        modifier = Modifier.padding(start = 24.dp, end = 16.dp, top = 14.dp, bottom = 4.dp),
    )
}

@Composable
private fun QuietLine(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = CedarColors.secondaryText,
        modifier = Modifier.padding(horizontal = 24.dp, vertical = 8.dp),
    )
}

/** "2 things need you", in the amber of what needs the person. */
@Composable
private fun NeedsYouLine(count: Int) {
    Text(
        plural(R.plurals.today_needs_you, count, count),
        style = MaterialTheme.typography.labelLarge,
        fontWeight = FontWeight.SemiBold,
        color = MaterialTheme.colorScheme.onSecondaryContainer,
        modifier = Modifier
            .padding(start = 16.dp, end = 16.dp, top = 6.dp)
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.secondaryContainer)
            .padding(horizontal = 12.dp, vertical = 5.dp),
    )
}

/** The box to ask: a tap opens the sheet with the text and its When. */
@Composable
private fun AskBar(enabled: Boolean, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 6.dp)
            .surfaceCard(radius = 28.dp)
            .clickable(enabled = enabled, onClick = onClick)
            .padding(start = 20.dp, end = 8.dp, top = 8.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Text(
            stringResource(R.string.today_ask_placeholder),
            Modifier.weight(1f),
            color = CedarColors.secondaryText,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            stringResource(R.string.today_when_now),
            style = MaterialTheme.typography.labelLarge,
            fontWeight = FontWeight.SemiBold,
            color = MaterialTheme.colorScheme.onPrimary,
            modifier = Modifier
                .clip(RoundedCornerShape(50))
                .background(if (enabled) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.primary.copy(alpha = 0.4f))
                .padding(horizontal = 14.dp, vertical = 9.dp),
        )
    }
}

/** Where a turn under now stands, as a small pill. */
@Composable
private fun StatePill(state: TurnState) {
    val (text, color) = when (state) {
        TurnState.WAITING -> stringResource(R.string.today_pill_waiting) to CedarColors.clay
        TurnState.ASKING -> stringResource(R.string.today_pill_asking) to CedarColors.clay
        else -> stringResource(R.string.today_pill_working) to MaterialTheme.colorScheme.primary
    }
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.SemiBold,
        color = color,
        maxLines = 1,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(color.copy(alpha = 0.15f))
            .padding(horizontal = 8.dp, vertical = 3.dp),
    )
}

/**
 * A turn under now. One that waits on an OK is allowed or denied here; a plan, a question and a
 * response still going open the chat, where the plan can be edited and the answer typed.
 */
@Composable
private fun NowCard(app: AppModel, turn: TimelineTurn, onAction: (suspend () -> Unit) -> Unit) {
    val state = turn.state
    var deciding by remember(turn.turnId) { mutableStateOf(false) }
    Column(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 16.dp, vertical = 6.dp)
            .surfaceCard(stroke = if (state.needsYou) CedarColors.clay else MaterialTheme.colorScheme.outlineVariant)
            .clickable { app.open(turn.conversationId) }
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(turn.label, Modifier.weight(1f), fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            StatePill(state)
        }
        when (state) {
            TurnState.WAITING -> turn.waits.forEach { wait ->
                val approval = wait.approvalId
                if (approval != null) {
                    Text(wait.reason?.takeIf { it.isNotBlank() } ?: stringResource(R.string.today_approval_waiting), style = MaterialTheme.typography.bodyMedium)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                        fun decide(allow: Boolean) {
                            deciding = true
                            onAction {
                                try {
                                    app.decideApproval(turn.turnId, approval, allow)
                                } finally {
                                    deciding = false
                                }
                            }
                        }
                        Button(onClick = { decide(true) }, enabled = !deciding, colors = needColors()) { Text(stringResource(R.string.allow)) }
                        OutlinedButton(onClick = { decide(false) }, enabled = !deciding) { Text(stringResource(R.string.deny)) }
                        if (deciding) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                    }
                } else {
                    Text(stringResource(R.string.today_plan_waiting), style = MaterialTheme.typography.bodyMedium)
                    val steps = wait.planSteps
                    if (steps.isEmpty()) {
                        // A server that doesn't send the steps: the plan is agreed to in the chat.
                        OutlinedButton(onClick = { app.open(turn.conversationId) }) { Text(stringResource(R.string.today_review_plan)) }
                    } else {
                        Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            steps.forEachIndexed { index, step ->
                                Text("${index + 1}. ${step.title}", style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText)
                            }
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Button(
                                onClick = {
                                    deciding = true
                                    onAction {
                                        try {
                                            app.agreePlan(turn.turnId, steps)
                                        } finally {
                                            deciding = false
                                        }
                                    }
                                },
                                enabled = !deciding,
                                colors = needColors(),
                            ) { Text(stringResource(R.string.today_go_ahead)) }
                            // Its steps are changed in the chat.
                            OutlinedButton(onClick = { app.open(turn.conversationId) }, enabled = !deciding) { Text(stringResource(R.string.today_change_plan)) }
                            if (deciding) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        }
                    }
                }
            }
            TurnState.ASKING -> {
                turn.line?.let { Text(it, style = MaterialTheme.typography.bodyMedium, maxLines = 4, overflow = TextOverflow.Ellipsis) }
                Button(onClick = { app.open(turn.conversationId) }, colors = needColors()) { Text(stringResource(R.string.answer)) }
            }
            else -> Row(verticalAlignment = Alignment.CenterVertically) {
                CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                Spacer(Modifier.width(8.dp))
                // What it is doing now and how far it has come, once its run has called a tool.
                val progress = turn.progress
                val doing = progress?.doing?.takeIf { it.isNotBlank() }?.let { toolLabel(it) }
                val where = if (progress != null && progress.steps > 0) {
                    listOfNotNull(doing, plural(R.plurals.today_steps_so_far, progress.steps, progress.steps)).joinToString(" · ")
                } else {
                    stringResource(R.string.today_working)
                }
                Text(where, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
                TextButton(
                    onClick = {
                        deciding = true
                        onAction {
                            try {
                                app.stopTurn(turn.turnId)
                            } finally {
                                deciding = false
                            }
                        }
                    },
                    enabled = !deciding,
                ) { Text(stringResource(R.string.stop)) }
            }
        }
    }
}

/**
 * What an agent's run held for the person's OK, under now: who wanted it (a tap opens that run's
 * outcome) and the outbox's own card, which approves or discards it.
 */
@Composable
private fun HeldEntry(app: AppModel, held: OutboxItem) {
    // As last heard of: a decision made here, or on the Agents tab, settles it at once.
    val current = app.heldCall(held.id) ?: held
    Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 6.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
        val origin = listOfNotNull(held.origin, held.createdAt?.let { shortTime(it) }).joinToString(" · ")
        if (origin.isNotEmpty()) {
            val turnId = held.turnId
            Text(
                origin,
                if (turnId != null) Modifier.clickable { app.showingOutcome = turnId } else Modifier,
                style = MaterialTheme.typography.labelLarge,
                color = if (turnId != null) MaterialTheme.colorScheme.primary else CedarColors.secondaryText,
            )
        }
        HeldCallView(
            note = null,
            item = current,
            gone = false,
            deciding = app.isDecidingHeldCall(held.id),
            error = app.heldCallError(held.id),
            onDecide = { approve -> app.decideHeldCall(held.id, approve) },
        )
    }
}

/** A turn that is over: when, what it was, and what it came to in a line. */
@Composable
private fun EarlierRow(turn: TimelineTurn, onClick: () -> Unit) {
    val small = MaterialTheme.typography.bodySmall
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 10.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        TimeColumn(shortTime(turn.createdAt))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (turn.agent != null) Icon(Icons.Outlined.SmartToy, null, Modifier.size(14.dp), tint = CedarColors.secondaryText)
                Text(
                    turn.label,
                    Modifier.weight(1f, fill = false),
                    fontWeight = if (turn.seen) FontWeight.Normal else FontWeight.SemiBold,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                when (turn.state) {
                    TurnState.FAILED -> Text(stringResource(R.string.today_failed), style = small, color = CedarColors.red, maxLines = 1)
                    TurnState.STOPPED -> Text(stringResource(R.string.today_stopped), style = small, color = CedarColors.secondaryText, maxLines = 1)
                    else -> {}
                }
            }
            turn.line?.let { Text(it, style = small, color = CedarColors.secondaryText, maxLines = 2, overflow = TextOverflow.Ellipsis) }
        }
    }
}

/** An agent ahead: when it runs next, or "any time" for one that something outside starts. */
@Composable
private fun AheadRow(agent: Agent, shownDay: LocalDate, onClick: () -> Unit) {
    val context = LocalContext.current
    val next = agent.nextRunAt
    val time = when {
        agent.trigger.byEvent || next == null -> stringResource(R.string.today_any_time)
        next.atZone(ZoneId.systemDefault()).toLocalDate() == shownDay -> shortTime(next)
        else -> skeletonDate(next, "EEEjm")
    }
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 16.dp, vertical = 5.dp),
        horizontalArrangement = Arrangement.spacedBy(12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        TimeColumn(time)
        // Dashed: it hasn't happened yet.
        Column(
            Modifier.weight(1f).dashedOutline(MaterialTheme.colorScheme.outline).padding(horizontal = 14.dp, vertical = 9.dp),
            verticalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            Text(agent.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(context.describe(agent.trigger), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

/** The river's left column: a time, or a few words standing in for one. */
@Composable
private fun TimeColumn(text: String) {
    Text(
        text,
        Modifier.width(64.dp).padding(top = 2.dp),
        style = MaterialTheme.typography.labelMedium,
        fontFamily = FontFamily.Monospace,
        color = CedarColors.secondaryText,
        maxLines = 2,
    )
}

// -- asking, with a When -------------------------------------------------------------------------

/**
 * The ask: the text and its When. Now sends it as a new chat, which opens (off the record, where
 * the server has private chats); Later, Every and When something changes make an agent of it,
 * with the few details each needs. Anything a trigger needs beyond these is the agent form's.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AskSheet(app: AppModel, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var text by rememberSaveable { mutableStateOf("") }
    var draft by remember { mutableStateOf(AskDraft.starting()) }
    var minutes by remember { mutableStateOf(draft.minutesBefore.toString()) }
    var sending by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    // The When the server read out of the sentence, said back until something is changed.
    var reading by remember { mutableStateOf(false) }
    var readAs by remember { mutableStateOf<String?>(null) }
    val context = LocalContext.current
    val noText = stringResource(R.string.today_problem_no_text)
    val readNow = stringResource(R.string.today_read_now)

    fun workOut() {
        if (text.isBlank()) {
            error = noText
            return
        }
        error = null
        reading = true
        scope.launch {
            try {
                val read = app.readWhen(text)
                val filled = draft.reading(read)
                draft = filled
                minutes = filled.minutesBefore.toString()
                val trigger = filled.trigger(java.util.TimeZone.getDefault().id)
                if (filled.whenKind != AskWhen.NOW && trigger != null) {
                    // What runs is the sentence without its timing.
                    if (read.content.isNotBlank()) text = read.content
                    readAs = context.getString(R.string.today_read_as, context.describe(trigger))
                } else {
                    readAs = readNow
                }
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = app.describe(e)
            } finally {
                reading = false
            }
        }
    }
    val problemTexts = mapOf(
        AskProblem.NO_TEXT to stringResource(R.string.today_problem_no_text),
        AskProblem.NO_TIME to stringResource(R.string.today_problem_no_time),
        AskProblem.TIME_PASSED to stringResource(R.string.today_problem_time_passed),
        AskProblem.NO_QUERY to stringResource(R.string.today_problem_no_query),
        AskProblem.BAD_MINUTES to stringResource(R.string.today_problem_bad_minutes),
    )

    fun send() {
        val problem = draft.problem(text)
        if (problem != null) {
            error = problemTexts[problem]
            return
        }
        error = null
        sending = true
        scope.launch {
            try {
                app.ask(text, draft)
                onDismiss()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = app.describe(e)
            } finally {
                sending = false
            }
        }
    }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(
            Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(start = 20.dp, end = 20.dp, bottom = 28.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            OutlinedTextField(
                value = text,
                onValueChange = { text = it.take(32_000) },
                placeholder = { Text(stringResource(R.string.today_ask_hint)) },
                minLines = 2,
                maxLines = 6,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(stringResource(R.string.today_when), Modifier.weight(1f), style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText)
                if (reading) CircularProgressIndicator(Modifier.size(16.dp), strokeWidth = 2.dp)
                TextButton(onClick = { workOut() }, enabled = !reading && !sending) { Text(stringResource(R.string.today_work_out_when)) }
            }
            readAs?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary) }
            ChipRow(
                options = listOf(
                    AskWhen.NOW to stringResource(R.string.today_when_now),
                    AskWhen.LATER to stringResource(R.string.today_when_later),
                    AskWhen.EVERY to stringResource(R.string.today_when_every),
                    AskWhen.WATCH to stringResource(R.string.today_when_watch),
                ),
                selected = draft.whenKind,
            ) {
                draft = draft.copy(whenKind = it)
                readAs = null
            }

            when (draft.whenKind) {
                AskWhen.NOW -> if (app.serverHasPrivateChats) {
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        Text(stringResource(R.string.today_private), Modifier.weight(1f))
                        Switch(checked = draft.private, onCheckedChange = { draft = draft.copy(private = it) })
                    }
                }
                AskWhen.LATER -> {
                    val at = draft.laterAt ?: LocalDateTime.now().plusHours(2)
                    AskDateRow(at.toLocalDate()) { draft = draft.copy(laterAt = LocalDateTime.of(it, at.toLocalTime())) }
                    AskTimeRow(at.toLocalTime()) { draft = draft.copy(laterAt = LocalDateTime.of(at.toLocalDate(), it)) }
                }
                AskWhen.EVERY -> {
                    // Read from the sentence and not one of the choices below: sent as it is, until one is picked.
                    draft.kept?.let { kept ->
                        Text(stringResource(R.string.today_read_kept, context.describe(kept)), style = MaterialTheme.typography.bodyMedium)
                    }
                    ChipRow(
                        options = listOf(
                            AskFrequency.DAILY to stringResource(R.string.today_every_daily),
                            AskFrequency.WEEKDAYS to stringResource(R.string.today_every_weekdays),
                            AskFrequency.WEEKLY to stringResource(R.string.today_every_weekly),
                        ),
                        selected = draft.frequency,
                    ) { draft = draft.copy(frequency = it).edited() }
                    if (draft.frequency == AskFrequency.WEEKLY) {
                        ChipRow(
                            options = DayOfWeek.values().map { AskDraft.weekday(it) to it.getDisplayName(TextStyle.SHORT_STANDALONE, Locale.getDefault()) },
                            selected = draft.weekday,
                        ) { draft = draft.copy(weekday = it).edited() }
                    }
                    val time = runCatching { LocalTime.parse(draft.time) }.getOrDefault(LocalTime.of(7, 0))
                    AskTimeRow(time) { draft = draft.copy(time = it.format(DateTimeFormatter.ofPattern("HH:mm"))).edited() }
                }
                AskWhen.WATCH -> {
                    ChipRow(
                        options = listOf(
                            AskWatch.EMAIL to stringResource(R.string.today_watch_email),
                            AskWatch.CALENDAR to stringResource(R.string.today_watch_calendar),
                            AskWatch.DEVICE to stringResource(R.string.today_watch_device),
                        ),
                        selected = draft.watch,
                    ) { draft = draft.copy(watch = it) }
                    when (draft.watch) {
                        AskWatch.EMAIL -> OutlinedTextField(
                            value = draft.query,
                            onValueChange = { draft = draft.copy(query = it.take(500)) },
                            label = { Text(stringResource(R.string.today_watch_query)) },
                            placeholder = { Text("from:bank subject:invoice") },
                            singleLine = true,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        AskWatch.CALENDAR -> OutlinedTextField(
                            value = minutes,
                            onValueChange = { typed ->
                                minutes = typed.filter { it.isDigit() }.take(4)
                                draft = draft.copy(minutesBefore = minutes.toIntOrNull() ?: 0)
                            },
                            label = { Text(stringResource(R.string.today_watch_minutes)) },
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                            modifier = Modifier.fillMaxWidth(),
                        )
                        AskWatch.DEVICE -> {}
                    }
                }
            }

            Text(
                stringResource(
                    when {
                        draft.isPrivate -> R.string.today_hint_private
                        draft.whenKind == AskWhen.NOW -> R.string.today_hint_now
                        draft.whenKind == AskWhen.LATER -> R.string.today_hint_later
                        draft.whenKind == AskWhen.EVERY -> R.string.today_hint_every
                        else -> R.string.today_hint_watch
                    },
                ),
                style = MaterialTheme.typography.bodySmall,
                color = CedarColors.secondaryText,
            )
            error?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.red) }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End), verticalAlignment = Alignment.CenterVertically) {
                if (sending) CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) }
                Button(onClick = { send() }, enabled = !sending && !reading) {
                    Text(
                        stringResource(
                            when (draft.whenKind) {
                                AskWhen.NOW -> R.string.send
                                AskWhen.LATER -> R.string.today_send_later
                                AskWhen.EVERY -> R.string.today_send_every
                                AskWhen.WATCH -> R.string.today_send_watch
                            },
                        ),
                    )
                }
            }
        }
    }
}

/** A few choices as chips in a row that scrolls when they don't fit. */
@Composable
private fun <T> ChipRow(options: List<Pair<T, String>>, selected: T, onSelect: (T) -> Unit) {
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        options.forEach { (value, label) ->
            FilterChip(selected = selected == value, onClick = { onSelect(value) }, label = { Text(label, maxLines = 1) })
        }
    }
}

/** A row that shows a value and opens something to change it, as the agent form's. */
@Composable
private fun AskValueRow(label: String, value: String, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text(label, Modifier.weight(1f))
        Text(value, color = CedarColors.secondaryText)
        Icon(Icons.Outlined.ArrowDropDown, null, tint = CedarColors.secondaryText)
    }
}

/** A time of day, changed with the system's clock. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AskTimeRow(time: LocalTime, onPick: (LocalTime) -> Unit) {
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    AskValueRow(stringResource(R.string.trigger_time), time.format(DateTimeFormatter.ofLocalizedTime(FormatStyle.SHORT))) { open = true }
    if (open) {
        val state = rememberTimePickerState(initialHour = time.hour, initialMinute = time.minute, is24Hour = DateFormat.is24HourFormat(context))
        AlertDialog(
            onDismissRequest = { open = false },
            text = { TimePicker(state) },
            confirmButton = {
                TextButton(onClick = {
                    open = false
                    onPick(LocalTime.of(state.hour, state.minute))
                }) { Text(stringResource(R.string.ok)) }
            },
            dismissButton = { TextButton(onClick = { open = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

/** A day, changed with the system's calendar. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun AskDateRow(date: LocalDate, onPick: (LocalDate) -> Unit) {
    var open by remember { mutableStateOf(false) }
    AskValueRow(stringResource(R.string.trigger_date), date.format(DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM))) { open = true }
    if (open) {
        // The picker counts days in UTC.
        val state = rememberDatePickerState(initialSelectedDateMillis = date.atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli())
        DatePickerDialog(
            onDismissRequest = { open = false },
            confirmButton = {
                TextButton(onClick = {
                    open = false
                    state.selectedDateMillis?.let { onPick(Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate()) }
                }) { Text(stringResource(R.string.ok)) }
            },
            dismissButton = { TextButton(onClick = { open = false }) { Text(stringResource(R.string.cancel)) } },
        ) { DatePicker(state) }
    }
}
