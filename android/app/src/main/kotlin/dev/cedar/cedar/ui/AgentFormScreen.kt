package dev.cedar.cedar.ui

import android.text.format.DateFormat
import androidx.activity.compose.BackHandler
import androidx.annotation.StringRes
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.ArrowDropDown
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Error
import androidx.compose.material.icons.outlined.ExpandLess
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.RadioButtonUnchecked
import androidx.compose.material.icons.outlined.Science
import androidx.compose.material.icons.outlined.Shield
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DatePicker
import androidx.compose.material3.DatePickerDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TimePicker
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDatePickerState
import androidx.compose.material3.rememberTimePickerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AgentFormRequest
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.AgentBody
import dev.cedar.core.AgentLimits
import dev.cedar.core.Change
import dev.cedar.core.NotificationStatus
import dev.cedar.core.Predicate
import dev.cedar.core.RecordSet
import dev.cedar.core.Span
import dev.cedar.core.ToolCatalog
import dev.cedar.core.ToolGroup
import dev.cedar.core.TriggerDraft
import dev.cedar.core.TriggerKind
import dev.cedar.core.TriggerTest
import kotlinx.coroutines.CancellationException
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
import java.util.TimeZone

/** Which tools an agent's runs get: what the server picks (or what the agent has), cedar's defaults, or a list. */
private enum class ToolsChoice { AUTO, DEFAULTS, CUSTOM }

/** Trying a trigger out. */
private sealed interface Trial {
    data object Idle : Trial
    data object Running : Trial
    data class Done(val result: TriggerTest) : Trial
    data class Failed(val message: String) : Trial
}

/**
 * The form an agent is made and edited with, as the web chat's: its name and prompt, what starts
 * it (a picker of triggers, and each one's own fields), Try it for the triggers that watch
 * something, then the model, how to be notified, the tools its runs may use, and for agents that
 * events start, how often they may run. Save sends it; closing asks before dropping what was typed.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentFormScreen(app: AppModel, request: AgentFormRequest) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val focus = LocalFocusManager.current
    val editing = request.agent
    val deviceZone = remember { TimeZone.getDefault().id }
    // A trigger from a newer server can't be edited here: it is shown, and left as it is on Save.
    val editable = remember { editing?.let { TriggerDraft.from(it.trigger, deviceZone) } }
    val locked = editing != null && editable == null
    val initial = remember { editable ?: TriggerDraft(timezone = deviceZone, date = LocalDate.now().plusDays(1).toString()) }

    var title by remember { mutableStateOf(editing?.title ?: request.title) }
    var prompt by remember { mutableStateOf(editing?.prompt ?: request.prompt) }
    var draft by remember { mutableStateOf(initial) }
    var model by remember { mutableStateOf(editing?.model.orEmpty()) }
    var reasoning by remember { mutableStateOf(editing?.reasoning.orEmpty()) }
    var notify by remember { mutableStateOf(editing?.notify.orEmpty()) }
    var toolsChoice by remember { mutableStateOf(ToolsChoice.AUTO) }
    var tools by remember { mutableStateOf(editing?.tools.orEmpty().toSet()) }
    var choosingTools by remember { mutableStateOf(false) }
    // Minutes between runs and runs a day; empty for the server's defaults.
    val savedMinInterval = remember { editing?.minIntervalSecs?.let { (it / 60).coerceAtLeast(1).toString() }.orEmpty() }
    val savedMaxRuns = remember { editing?.maxRunsPerDay?.toString().orEmpty() }
    var minInterval by remember { mutableStateOf(savedMinInterval) }
    var maxRuns by remember { mutableStateOf(savedMaxRuns) }

    var catalog by remember { mutableStateOf<ToolCatalog?>(null) }
    var recordSets by remember { mutableStateOf<List<RecordSet>>(emptyList()) }
    var notifications by remember { mutableStateOf<NotificationStatus?>(null) }
    var trial by remember { mutableStateOf<Trial>(Trial.Idle) }
    var saving by remember { mutableStateOf(false) }
    var problem by remember { mutableStateOf<String?>(null) }
    var confirmingDiscard by remember { mutableStateOf(false) }
    var touched by remember { mutableStateOf(false) }

    val limits = app.agentLimits
    val byEvent = if (locked) editing?.trigger?.byEvent == true else draft.kind.byEvent
    val personalTool = draft.kind == TriggerKind.CONDITION && draft.sourceType == TriggerDraft.SOURCE_TOOL && catalog?.isPersonal(draft.tool) == true
    val canSave = title.isNotBlank() && prompt.isNotBlank() && !saving

    fun edit(change: TriggerDraft) {
        draft = change
        touched = true
        // What was tried is no longer what is shown.
        trial = Trial.Idle
    }

    fun close() {
        if (touched) confirmingDiscard = true else app.agentForm = null
    }

    // What the pickers offer; the form works without any of it (a failure leaves that picker short).
    LaunchedEffect(Unit) {
        val client = app.client ?: return@LaunchedEffect
        launch { catalog = runCatching { client.tools() }.getOrNull() }
        launch { recordSets = runCatching { client.records() }.getOrNull()?.sets.orEmpty() }
        launch { notifications = runCatching { client.notifications() }.getOrNull() }
        if (app.agents.isEmpty()) launch { app.refreshAgents() }
    }

    /** The trigger as the server takes it, or `null` after saying what is missing. */
    fun built(): dev.cedar.core.Trigger? {
        val now = runCatching { LocalDateTime.now(ZoneId.of(draft.timezone)) }.getOrNull()
        return when (val b = draft.build(limits, personalTool = personalTool, now = now)) {
            is TriggerDraft.Built.Ready -> b.trigger
            is TriggerDraft.Built.Invalid -> {
                problem = context.describe(b.problem)
                null
            }
        }
    }

    fun tryIt() {
        focus.clearFocus()
        val trigger = built() ?: return
        val client = app.client ?: return
        trial = Trial.Running
        scope.launch {
            trial = try {
                Trial.Done(client.testTrigger(trigger))
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                Trial.Failed(context.describe(e))
            }
        }
    }

    fun save() {
        focus.clearFocus()
        // An agent being edited is sent only what was changed: a trigger sent again as it was
        // could read as a new one (and forget what it had seen), and a model or a notification
        // method that has gone since would be refused.
        val trigger = if (locked || (editing != null && draft == initial)) null else built() ?: return
        fun limit(text: String, saved: String, range: IntRange, scale: Int): Change<Int>? {
            if (text.trim() == saved) return Change.Keep
            if (text.isBlank()) return Change.Clear
            val n = text.trim().toIntOrNull()?.let { it.toLong() * scale }?.takeIf { it >= range.first && it <= range.last } ?: return null
            return Change.Set(n.toInt())
        }
        val interval = if (byEvent) limit(minInterval, savedMinInterval, AgentLimits.MIN_INTERVAL_SECS_RANGE, 60) else Change.Keep
        val runs = if (byEvent) limit(maxRuns, savedMaxRuns, AgentLimits.MAX_RUNS_PER_DAY_RANGE, 1) else Change.Keep
        if (interval == null || runs == null) {
            problem = context.getString(
                R.string.agent_limits_problem,
                AgentLimits.MIN_INTERVAL_SECS_RANGE.first / 60, AgentLimits.MIN_INTERVAL_SECS_RANGE.last / 60,
                AgentLimits.MAX_RUNS_PER_DAY_RANGE.first, AgentLimits.MAX_RUNS_PER_DAY_RANGE.last,
            )
            return
        }
        val modelChanged = editing == null || model != editing.model.orEmpty() || reasoning != editing.reasoning.orEmpty()
        val body = AgentBody(
            title = title.trim(),
            prompt = prompt.trim(),
            // "" is cedar's default model; a new agent just leaves it out.
            model = if (modelChanged) model.ifEmpty { if (editing == null) null else "" } else null,
            reasoning = if (modelChanged) reasoning.ifEmpty { if (editing == null) null else "" } else null,
            conversationId = if (editing == null) request.conversationId else null,
            tools = when (toolsChoice) {
                ToolsChoice.AUTO -> Change.Keep
                ToolsChoice.DEFAULTS -> Change.Clear
                ToolsChoice.CUSTOM -> Change.Set(tools.sorted())
            },
            notify = when {
                notify == editing?.notify.orEmpty() -> Change.Keep
                notify.isEmpty() -> Change.Clear
                else -> Change.Set(notify)
            },
            trigger = trigger,
            timezone = deviceZone,
            minIntervalSecs = interval,
            maxRunsPerDay = runs,
        )
        saving = true
        scope.launch {
            try {
                val saved = app.saveAgent(editing?.id, body)
                app.agentForm = null
                // Its page: a new webhook's secret is shown there, this once.
                app.openAgent(saved.id)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                problem = context.describe(e)
            } finally {
                saving = false
            }
        }
    }

    BackHandler { close() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(if (editing == null) R.string.new_agent else R.string.edit_agent)) },
                navigationIcon = { IconButton(onClick = ::close) { Icon(Icons.Filled.Close, stringResource(R.string.cancel)) } },
                actions = {
                    if (saving) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    TextButton(onClick = ::save, enabled = canSave) { Text(stringResource(R.string.save)) }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 640.dp).fillMaxWidth().verticalScroll(rememberScrollState()).imePadding().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                OutlinedTextField(
                    value = title,
                    onValueChange = { title = it; touched = true },
                    label = { Text(stringResource(R.string.agent_name)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = prompt,
                    onValueChange = { prompt = it; touched = true },
                    label = { Text(stringResource(R.string.agent_prompt)) },
                    minLines = 4,
                    maxLines = 14,
                    keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences),
                    modifier = Modifier.fillMaxWidth(),
                )
                Footnote(stringResource(if (request.conversationId != null && editing == null) R.string.agent_prompt_from_chat else R.string.agent_prompt_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.agent_trigger))
                if (locked) {
                    Text(context.describe(editing!!.trigger))
                    Footnote(stringResource(R.string.trigger_unknown_note))
                } else {
                    Picker(
                        label = stringResource(R.string.agent_starts),
                        value = stringResource(triggerKindLabel(draft.kind)),
                        options = TriggerKind.entries.map { it.id to stringResource(triggerKindLabel(it)) },
                    ) { id -> TriggerKind.of(id)?.let { edit(draft.copy(kind = it)) } }
                    TriggerFields(draft, limits, catalog, recordSets, personalTool, ::edit)
                    if (draft.kind.watches) {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            OutlinedButton(onClick = ::tryIt, enabled = trial != Trial.Running) {
                                Icon(Icons.Outlined.Science, null, Modifier.size(18.dp))
                                Spacer(Modifier.width(6.dp))
                                Text(stringResource(R.string.try_it))
                            }
                            if (trial == Trial.Running) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        }
                        TrialResult(trial)
                    }
                }
                if (byEvent) SafetyNote(stringResource(R.string.agent_safety_events))
                if (personalTool) SafetyNote(stringResource(R.string.agent_safety_personal_tool))

                HorizontalDivider()
                Section(stringResource(R.string.section_ai_model))
                val models = app.models
                val selected = models.firstOrNull { it.id == model }
                val serverDefault = stringResource(R.string.server_default)
                Picker(
                    label = stringResource(R.string.ai_model),
                    value = selected?.label ?: if (model.isEmpty()) serverDefault else model,
                    options = listOf("" to serverDefault) + models.map { it.id to it.label },
                ) { id ->
                    // A level the new model doesn't offer is dropped.
                    val levels = models.firstOrNull { it.id == id }?.reasoningLevels.orEmpty()
                    model = id
                    if (reasoning !in levels) reasoning = ""
                    touched = true
                }
                val levels = selected?.reasoningLevels.orEmpty()
                if (levels.isNotEmpty()) {
                    // The levels are the server's words (low, medium, high).
                    val default = stringResource(R.string.reasoning_default)
                    Picker(
                        label = stringResource(R.string.reasoning_effort),
                        value = reasoning.replaceFirstChar { it.uppercase() }.ifEmpty { default },
                        options = listOf("" to default) + levels.map { it to it.replaceFirstChar { c -> c.uppercase() } },
                    ) {
                        reasoning = it
                        touched = true
                    }
                }

                HorizontalDivider()
                Section(stringResource(R.string.section_notifications))
                val defaultMethod = notifications?.defaultMethod ?: "none"
                val followDefault = if (defaultMethod == "none") stringResource(R.string.notify_default_none) else stringResource(R.string.notify_default_method, context.notifyMethodLabel(defaultMethod))
                val methods = buildList {
                    add("" to followDefault)
                    add("none" to context.notifyMethodLabel("none"))
                    notifications?.configured.orEmpty().filter { it != "none" }.forEach { add(it to context.notifyMethodLabel(it)) }
                    // The agent's own choice stays on offer, whatever became of its setup.
                    if (notify.isNotEmpty() && none { it.first == notify }) add(notify to context.notifyMethodLabel(notify))
                }
                Picker(
                    label = stringResource(R.string.agent_notify),
                    value = methods.firstOrNull { it.first == notify }?.second ?: notify,
                    options = methods,
                ) {
                    notify = it
                    touched = true
                }
                Footnote(stringResource(R.string.agent_notify_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.agent_tools))
                val auto = stringResource(
                    when {
                        editing != null -> R.string.agent_tools_keep
                        byEvent -> R.string.agent_tools_careful
                        request.conversationId != null -> R.string.agent_tools_conversation
                        else -> R.string.agent_tools_defaults
                    },
                )
                // For a new agent that a clock starts, out of no conversation, the server's pick is the defaults.
                val offersDefaults = editing != null || byEvent || request.conversationId != null
                val choices = buildList {
                    add(ToolsChoice.AUTO.name to auto)
                    if (offersDefaults) add(ToolsChoice.DEFAULTS.name to stringResource(R.string.agent_tools_defaults))
                    add(ToolsChoice.CUSTOM.name to stringResource(R.string.agent_tools_choose))
                }
                Picker(
                    label = stringResource(R.string.agent_tools_use),
                    value = choices.firstOrNull { it.first == toolsChoice.name }?.second ?: auto,
                    options = choices,
                ) {
                    toolsChoice = ToolsChoice.valueOf(it)
                    touched = true
                    if (toolsChoice == ToolsChoice.CUSTOM) choosingTools = true
                }
                when (toolsChoice) {
                    ToolsChoice.AUTO -> if (editing != null) {
                        Footnote(editing.tools?.let { plural(R.plurals.agent_tools_count, it.size, it.size) } ?: stringResource(R.string.agent_tools_defaults))
                    }
                    ToolsChoice.DEFAULTS -> {}
                    ToolsChoice.CUSTOM -> Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedButton(onClick = { choosingTools = true }) { Text(stringResource(R.string.agent_tools_choose)) }
                        Text(plural(R.plurals.agent_tools_count, tools.size, tools.size), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                    }
                }
                Footnote(stringResource(if (byEvent) R.string.agent_tools_footnote_events else R.string.agent_tools_footnote))

                if (byEvent) {
                    HorizontalDivider()
                    Section(stringResource(R.string.agent_limits))
                    NumberField(
                        stringResource(R.string.agent_min_interval), minInterval,
                        placeholder = (limits.minIntervalSecs / 60).coerceAtLeast(1).toString(),
                    ) { minInterval = it; touched = true }
                    NumberField(stringResource(R.string.agent_max_runs), maxRuns, placeholder = limits.maxRunsPerDay.toString()) { maxRuns = it; touched = true }
                    Footnote(stringResource(R.string.agent_limits_footnote))
                }
                Spacer(Modifier.size(24.dp))
            }
        }
    }

    if (choosingTools) {
        ToolsDialog(catalog, tools, onDismiss = { choosingTools = false }) {
            tools = it
            touched = true
        }
    }
    if (confirmingDiscard) {
        AlertDialog(
            onDismissRequest = { confirmingDiscard = false },
            title = { Text(stringResource(R.string.discard_changes_title)) },
            text = { Text(stringResource(R.string.agent_discard_text)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmingDiscard = false
                    app.agentForm = null
                }) { Text(stringResource(R.string.discard_changes), color = CedarColors.red) }
            },
            dismissButton = { TextButton(onClick = { confirmingDiscard = false }) { Text(stringResource(R.string.keep_editing)) } },
        )
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), problem) { problem = null }
}

/** The fields of the kind of trigger picked. */
@Composable
private fun TriggerFields(
    draft: TriggerDraft,
    limits: AgentLimits,
    catalog: ToolCatalog?,
    recordSets: List<RecordSet>,
    personalTool: Boolean,
    edit: (TriggerDraft) -> Unit,
) {
    val context = LocalContext.current
    when (draft.kind) {
        TriggerKind.TIME -> {
            Picker(
                label = stringResource(R.string.trigger_frequency),
                value = stringResource(frequencyLabel(draft.frequency)),
                options = TriggerDraft.FREQUENCIES.map { it to stringResource(frequencyLabel(it)) },
            ) { edit(draft.copy(frequency = it)) }
            if (draft.frequency == "weekly") {
                val days = (0..6).map { it.toString() to weekdayName(it) }
                Picker(label = stringResource(R.string.trigger_weekday), value = weekdayName(draft.weekday), options = days) {
                    edit(draft.copy(weekday = it.toIntOrNull() ?: 0))
                }
            }
            TimeRow(draft.time) { edit(draft.copy(time = it)) }
            ZoneRow(draft, edit)
        }
        TriggerKind.ONCE -> {
            DateRow(draft.date) { edit(draft.copy(date = it)) }
            TimeRow(draft.time) { edit(draft.copy(time = it)) }
            ZoneRow(draft, edit)
            Footnote(stringResource(R.string.trigger_once_footnote))
        }
        TriggerKind.INTERVAL -> {
            SpanField(stringResource(R.string.trigger_every), draft.every, draft.everyUnit) { n, unit -> edit(draft.copy(every = n, everyUnit = unit)) }
            Footnote(stringResource(R.string.trigger_interval_footnote, context.duration(limits.minIntervalMinutes)))
        }
        TriggerKind.MONTHLY -> {
            NumberField(stringResource(R.string.trigger_day_of_month), draft.day, placeholder = "1") { edit(draft.copy(day = it)) }
            TimeRow(draft.time) { edit(draft.copy(time = it)) }
            ZoneRow(draft, edit)
            Footnote(stringResource(R.string.trigger_monthly_footnote))
        }
        TriggerKind.CONDITION -> ConditionFields(draft, limits, catalog, recordSets, personalTool, edit)
        TriggerKind.WEBHOOK -> Footnote(stringResource(R.string.trigger_webhook_footnote))
        TriggerKind.EMAIL -> {
            PlainField(stringResource(R.string.trigger_email_query), draft.query, placeholder = "from:someone@example.com is:unread") { edit(draft.copy(query = it)) }
            Footnote(stringResource(R.string.trigger_email_footnote))
            CheckEvery(draft, edit)
        }
        TriggerKind.CALENDAR -> {
            PlainField(stringResource(R.string.trigger_calendar_filter_label), draft.filter, placeholder = stringResource(R.string.optional)) { edit(draft.copy(filter = it)) }
            SwitchLine(stringResource(R.string.trigger_calendar_before_label), draft.beforeEvents) { edit(draft.copy(beforeEvents = it)) }
            if (draft.beforeEvents) {
                NumberField(stringResource(R.string.trigger_calendar_minutes_before), draft.minutesBefore, placeholder = "30") { edit(draft.copy(minutesBefore = it)) }
            }
            SwitchLine(stringResource(R.string.trigger_calendar_new_label), draft.newEvents) { edit(draft.copy(newEvents = it)) }
            Footnote(stringResource(R.string.trigger_calendar_footnote))
            CheckEvery(draft, edit)
        }
    }
}

@Composable
private fun ConditionFields(
    draft: TriggerDraft,
    limits: AgentLimits,
    catalog: ToolCatalog?,
    recordSets: List<RecordSet>,
    personalTool: Boolean,
    edit: (TriggerDraft) -> Unit,
) {
    val context = LocalContext.current
    val sources = listOf(
        TriggerDraft.SOURCE_URL to stringResource(R.string.source_url),
        TriggerDraft.SOURCE_TOOL to stringResource(R.string.source_tool),
        TriggerDraft.SOURCE_RECORDS to stringResource(R.string.source_records),
    )
    Picker(
        label = stringResource(R.string.trigger_source),
        value = sources.firstOrNull { it.first == draft.sourceType }?.second ?: draft.sourceType,
        options = sources,
    ) { edit(draft.copy(sourceType = it)) }
    when (draft.sourceType) {
        TriggerDraft.SOURCE_TOOL -> {
            val pollable = catalog?.pollable.orEmpty()
            // The tool it has stays on offer, whatever became of it.
            val names = (pollable.map { it.second.name } + listOfNotNull(draft.tool.takeIf { it.isNotEmpty() })).distinct()
            if (names.isEmpty()) {
                Footnote(stringResource(if (catalog == null) R.string.source_tools_loading else R.string.source_tools_none))
            } else {
                Picker(
                    label = stringResource(R.string.source_tool),
                    value = draft.tool.ifEmpty { stringResource(R.string.choose) },
                    options = names.map { name ->
                        val group = pollable.firstOrNull { it.second.name == name }?.first
                        name to if (group != null) "$name · ${group.label}" else name
                    },
                ) { edit(draft.copy(tool = it)) }
                pollable.firstOrNull { it.second.name == draft.tool }?.second?.description?.takeIf { it.isNotBlank() }?.let { Footnote(it) }
            }
            PlainField(stringResource(R.string.source_tool_input), draft.toolInput, placeholder = "{}", monospace = true, lines = 3) { edit(draft.copy(toolInput = it)) }
            Footnote(stringResource(R.string.source_tool_input_footnote))
        }
        TriggerDraft.SOURCE_RECORDS -> {
            val names = (recordSets.map { it.name } + listOfNotNull(draft.recordSet.takeIf { it.isNotEmpty() })).distinct()
            if (names.isEmpty()) {
                Footnote(stringResource(R.string.source_records_none))
            } else {
                Picker(
                    label = stringResource(R.string.source_record_set),
                    value = draft.recordSet.ifEmpty { stringResource(R.string.choose) },
                    options = names.map { it to it },
                ) { edit(draft.copy(recordSet = it)) }
            }
            Footnote(stringResource(R.string.source_records_footnote))
        }
        else -> {
            PlainField(stringResource(R.string.source_url_address), draft.url, placeholder = "https://", keyboard = KeyboardType.Uri) { edit(draft.copy(url = it)) }
            val formats = listOf(
                "" to stringResource(R.string.source_url_auto),
                "json" to stringResource(R.string.source_url_json),
                "text" to stringResource(R.string.source_url_text),
            )
            Picker(
                label = stringResource(R.string.source_url_read_as),
                value = formats.firstOrNull { it.first == draft.urlFormat }?.second ?: draft.urlFormat,
                options = formats,
            ) { edit(draft.copy(urlFormat = it)) }
        }
    }
    PlainField(
        stringResource(R.string.trigger_extract), draft.extract,
        placeholder = if (draft.sourceType == TriggerDraft.SOURCE_RECORDS) "count" else "price.eur",
        monospace = true,
    ) { edit(draft.copy(extract = it)) }
    Footnote(stringResource(R.string.trigger_extract_footnote))
    Picker(
        label = stringResource(R.string.trigger_test),
        value = opLabel(draft.op)?.let { stringResource(it) } ?: draft.op,
        // A test from a newer server stays on offer under its own name.
        options = (Predicate.OPS + draft.op).distinct().map { op -> op to (opLabel(op)?.let { stringResource(it) } ?: op) },
    ) { edit(draft.copy(op = it)) }
    if (draft.op != Predicate.CHANGES) {
        PlainField(
            stringResource(R.string.trigger_value), draft.value,
            keyboard = if (draft.op in Predicate.THRESHOLDS) KeyboardType.Decimal else KeyboardType.Text,
        ) { edit(draft.copy(value = it)) }
    }
    Footnote(stringResource(if (draft.op == Predicate.CHANGES) R.string.trigger_changes_footnote else R.string.trigger_holds_footnote))
    CheckEvery(draft, edit)
    if (personalTool) Footnote(stringResource(R.string.trigger_personal_check_footnote, context.duration(limits.personalCheckMinutes)))
}

/** How often a watching trigger checks. */
@Composable
private fun CheckEvery(draft: TriggerDraft, edit: (TriggerDraft) -> Unit) {
    SpanField(stringResource(R.string.trigger_check_every), draft.check, draft.checkUnit) { n, unit -> edit(draft.copy(check = n, checkUnit = unit)) }
}

@StringRes
private fun frequencyLabel(frequency: String): Int = when (frequency) {
    "weekdays" -> R.string.frequency_weekdays
    "weekly" -> R.string.frequency_weekly
    else -> R.string.frequency_daily
}

/** Monday (0) to Sunday (6), in the user's language. */
private fun weekdayName(index: Int): String =
    DayOfWeek.of(index.coerceIn(0, 6) + 1).getDisplayName(TextStyle.FULL_STANDALONE, Locale.getDefault()).replaceFirstChar { it.titlecase(Locale.getDefault()) }

@StringRes
private fun spanLabel(unit: Span): Int = when (unit) {
    Span.MINUTES -> R.string.span_minutes
    Span.HOURS -> R.string.span_hours
    Span.DAYS -> R.string.span_days
}

/** The time zone a clock trigger's times are in: this device's, or the one the agent was made with. */
@Composable
private fun ZoneRow(draft: TriggerDraft, edit: (TriggerDraft) -> Unit) {
    val device = remember { TimeZone.getDefault().id }
    val original = remember { draft.timezone }
    if (original == device) {
        Footnote(stringResource(R.string.trigger_zone, device))
    } else {
        Picker(
            label = stringResource(R.string.trigger_zone_label),
            value = draft.timezone,
            options = listOf(original to original, device to device),
        ) { edit(draft.copy(timezone = it)) }
    }
}

/** A row that shows a value and opens something to change it. */
@Composable
private fun ValueRow(label: String, value: String, onClick: () -> Unit) {
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

/** A time of day, `HH:MM`, changed with the system's clock. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun TimeRow(time: String, onPick: (String) -> Unit) {
    val context = LocalContext.current
    var open by remember { mutableStateOf(false) }
    val current = TriggerDraft.clock(time)?.let { LocalTime.parse(it) } ?: LocalTime.of(8, 0)
    ValueRow(stringResource(R.string.trigger_time), clockText(TriggerDraft.clock(time) ?: time)) { open = true }
    if (open) {
        val state = rememberTimePickerState(initialHour = current.hour, initialMinute = current.minute, is24Hour = DateFormat.is24HourFormat(context))
        AlertDialog(
            onDismissRequest = { open = false },
            text = { TimePicker(state) },
            confirmButton = {
                TextButton(onClick = {
                    open = false
                    onPick(TriggerDraft.clock("${state.hour}:${state.minute.toString().padStart(2, '0')}") ?: time)
                }) { Text(stringResource(R.string.ok)) }
            },
            dismissButton = { TextButton(onClick = { open = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}

/** A day, `YYYY-MM-DD`, changed with the system's calendar. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun DateRow(date: String, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val current = runCatching { LocalDate.parse(date) }.getOrNull()
    ValueRow(
        stringResource(R.string.trigger_date),
        current?.format(DateTimeFormatter.ofLocalizedDate(FormatStyle.MEDIUM)) ?: stringResource(R.string.choose),
    ) { open = true }
    if (open) {
        // The picker counts days in UTC.
        val state = rememberDatePickerState(initialSelectedDateMillis = (current ?: LocalDate.now()).atStartOfDay(ZoneOffset.UTC).toInstant().toEpochMilli())
        DatePickerDialog(
            onDismissRequest = { open = false },
            confirmButton = {
                TextButton(onClick = {
                    open = false
                    state.selectedDateMillis?.let { onPick(Instant.ofEpochMilli(it).atZone(ZoneOffset.UTC).toLocalDate().toString()) }
                }) { Text(stringResource(R.string.ok)) }
            },
            dismissButton = { TextButton(onClick = { open = false }) { Text(stringResource(R.string.cancel)) } },
        ) { DatePicker(state) }
    }
}

@Composable
private fun PlainField(
    label: String,
    value: String,
    placeholder: String? = null,
    monospace: Boolean = false,
    lines: Int = 1,
    keyboard: KeyboardType = KeyboardType.Text,
    onChange: (String) -> Unit,
) {
    OutlinedTextField(
        value = value,
        onValueChange = onChange,
        label = { Text(label) },
        placeholder = placeholder?.let { { Text(it) } },
        singleLine = lines == 1,
        minLines = lines,
        maxLines = if (lines == 1) 1 else 10,
        textStyle = if (monospace) MaterialTheme.typography.bodyMedium.copy(fontFamily = FontFamily.Monospace) else MaterialTheme.typography.bodyLarge,
        keyboardOptions = KeyboardOptions(keyboardType = keyboard, autoCorrectEnabled = false, capitalization = KeyboardCapitalization.None),
        modifier = Modifier.fillMaxWidth(),
    )
}

@Composable
private fun NumberField(label: String, value: String, modifier: Modifier = Modifier, placeholder: String? = null, onChange: (String) -> Unit) {
    OutlinedTextField(
        value = value,
        onValueChange = { text -> onChange(text.filter { it.isDigit() }.take(6)) },
        label = { Text(label) },
        placeholder = placeholder?.let { { Text(it) } },
        singleLine = true,
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
        modifier = modifier.fillMaxWidth(),
    )
}

/** A number and its unit: minutes, hours or days. */
@Composable
private fun SpanField(label: String, value: String, unit: Span, onChange: (String, Span) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        NumberField(label, value, modifier = Modifier.weight(1f)) { onChange(it, unit) }
        Box {
            OutlinedButton(onClick = { open = true }) {
                Text(stringResource(spanLabel(unit)))
                Icon(Icons.Outlined.ArrowDropDown, null)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
                Span.entries.forEach { option ->
                    DropdownMenuItem(text = { Text(stringResource(spanLabel(option))) }, onClick = {
                        open = false
                        onChange(value, option)
                    })
                }
            }
        }
    }
}

@Composable
private fun SwitchLine(label: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(label, Modifier.weight(1f))
        Switch(checked = on, onCheckedChange = onChange)
    }
}

/** Why to be careful with what this agent may do. */
@Composable
private fun SafetyNote(text: String) {
    Row(
        Modifier.fillMaxWidth().surfaceCard(radius = 12.dp, fill = CedarColors.clay.copy(alpha = 0.08f), stroke = CedarColors.clay.copy(alpha = 0.3f)).padding(12.dp),
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(Icons.Outlined.Shield, null, Modifier.size(18.dp), tint = CedarColors.clay)
        Text(text, style = MaterialTheme.typography.bodySmall)
    }
}

/** What trying the trigger out found: the value, whether a run would start, and the start of what the check returned. */
@Composable
private fun TrialResult(trial: Trial) {
    when (trial) {
        Trial.Idle, Trial.Running -> {}
        is Trial.Failed -> TrialLine(Icons.Outlined.Error, trial.message, CedarColors.red)
        is Trial.Done -> {
            val result = trial.result
            Column(Modifier.fillMaxWidth().surfaceCard(radius = 12.dp).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                if (result.ok) {
                    if (result.valueText.isNotEmpty()) {
                        Column {
                            Text(stringResource(R.string.trial_value), style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText)
                            SelectionContainer { Text(result.valueText, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodyMedium, maxLines = 8) }
                        }
                    }
                    if (result.wouldFire) {
                        TrialLine(Icons.Outlined.CheckCircle, result.summary?.takeIf { it.isNotBlank() }?.let { stringResource(R.string.trial_would_start_because, it) } ?: stringResource(R.string.trial_would_start), CedarColors.green)
                    } else {
                        TrialLine(Icons.Outlined.RadioButtonUnchecked, stringResource(R.string.trial_would_not_start), CedarColors.secondaryText)
                    }
                } else {
                    TrialLine(Icons.Outlined.Error, result.error?.takeIf { it.isNotBlank() } ?: stringResource(R.string.unknown_error), CedarColors.red)
                }
                result.sample?.takeIf { it.text.isNotEmpty() }?.let { sample ->
                    Text(
                        stringResource(if (sample.cut) R.string.trial_sample_cut else R.string.trial_sample),
                        style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText,
                    )
                    // What came back, to find a path in.
                    Box(Modifier.fillMaxWidth().heightIn(max = 240.dp).verticalScroll(rememberScrollState()).horizontalScroll(rememberScrollState())) {
                        SelectionContainer { Text(sample.text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall) }
                    }
                }
            }
        }
    }
}

@Composable
private fun TrialLine(icon: ImageVector, text: String, color: Color) {
    Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(icon, null, Modifier.size(16.dp), tint = color)
        Text(text, style = MaterialTheme.typography.bodySmall, color = color)
    }
}

/**
 * The tools an agent's runs may use: a whole group, or single tools of it. Names the catalog
 * doesn't list (a server's tools found only during a run, say) stay picked as they are.
 */
@Composable
private fun ToolsDialog(catalog: ToolCatalog?, picked: Set<String>, onDismiss: () -> Unit, onChange: (Set<String>) -> Unit) {
    var expanded by remember { mutableStateOf<Set<String>>(emptySet()) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.agent_tools)) },
        text = {
            Column(Modifier.heightIn(max = 460.dp).verticalScroll(rememberScrollState())) {
                val groups = catalog?.groups.orEmpty()
                if (groups.isEmpty()) {
                    Text(stringResource(if (catalog == null) R.string.source_tools_loading else R.string.agent_tools_none), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                }
                for (group in groups) {
                    val whole = group.id in picked
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = whole, onCheckedChange = { on -> onChange(if (on) picked + group.id - group.tools.map { it.name }.toSet() else picked - group.id) })
                        Column(Modifier.weight(1f)) {
                            Text(group.label, fontWeight = FontWeight.Medium)
                            groupNote(group)?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                        }
                        if (group.tools.isNotEmpty()) {
                            val open = group.id in expanded
                            IconButton(onClick = { expanded = if (open) expanded - group.id else expanded + group.id }) {
                                Icon(if (open) Icons.Outlined.ExpandLess else Icons.Outlined.ExpandMore, stringResource(if (open) R.string.show_less else R.string.show_more))
                            }
                        }
                    }
                    if (group.id in expanded) {
                        for (tool in group.tools) {
                            Row(Modifier.padding(start = 24.dp), verticalAlignment = Alignment.CenterVertically) {
                                Checkbox(
                                    checked = whole || tool.name in picked,
                                    enabled = !whole,
                                    onCheckedChange = { on -> onChange(if (on) picked + tool.name else picked - tool.name) },
                                )
                                Column(Modifier.weight(1f)) {
                                    Text(tool.name, style = MaterialTheme.typography.bodyMedium, fontFamily = FontFamily.Monospace)
                                    if (tool.writes) Text(stringResource(R.string.agent_tool_writes), style = MaterialTheme.typography.labelSmall, color = CedarColors.clay)
                                }
                            }
                        }
                    }
                }
                val known = groups.flatMap { group -> group.tools.map { it.name } + group.id }.toSet()
                val others = picked.filter { it !in known }.sorted()
                for (name in others) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = true, onCheckedChange = { onChange(picked - name) })
                        Text(name, style = MaterialTheme.typography.bodyMedium, fontFamily = FontFamily.Monospace)
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.done)) } },
    )
}

/** What to know about a group before picking it: it holds your own tools, or it can't be used right now. */
@Composable
private fun groupNote(group: ToolGroup): String? = when {
    group.isPersonal -> stringResource(R.string.agent_tools_personal)
    !group.available -> group.reason?.takeIf { it.isNotBlank() } ?: stringResource(R.string.not_available)
    else -> null
}
