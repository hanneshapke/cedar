package dev.cedar.cedar.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Pause
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Star
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AgentFormRequest
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.Agent
import dev.cedar.core.AgentRun
import dev.cedar.core.Hook
import dev.cedar.core.Trigger
import dev.cedar.core.WatchSource
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * One agent: what starts it and what it is doing, Run now and Pause or Resume, a webhook's
 * address and secret, its prompt, and its runs, newest first, each with what started it. Edit and
 * Delete are in the top bar. Tapping a run opens it as a conversation over this page.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun AgentDetailScreen(app: AppModel, agentId: String, onBack: (() -> Unit)?) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var actionError by remember { mutableStateOf<String?>(null) }
    var deleting by remember { mutableStateOf(false) }
    var rotating by remember { mutableStateOf(false) }
    val agent = app.agent(agentId)
    val runs = app.agentRuns[agentId]

    fun act(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // The server's own words for a refusal: "a run of this agent is still going; stop it first".
                actionError = context.describe(e)
            }
        }
    }

    LaunchedEffect(agentId) { app.loadAgent(agentId) }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(agent?.title ?: stringResource(R.string.agent), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = {
                    if (onBack != null) IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
                },
                actions = {
                    if (agent != null) {
                        IconButton(onClick = { app.agentForm = AgentFormRequest(agent = agent) }) { Icon(Icons.Outlined.Edit, stringResource(R.string.edit)) }
                        IconButton(onClick = { deleting = true }) { Icon(Icons.Outlined.Delete, stringResource(R.string.delete), tint = CedarColors.red) }
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            LazyColumn(Modifier.widthIn(max = 720.dp).fillMaxSize()) {
                app.agentError?.let { error -> item { ErrorLine(error) } }
                if (agent == null) {
                    if (app.agentError == null) item { Loading() }
                    return@LazyColumn
                }
                item(key = "summary") { Summary(agent, onRun = { act { app.runNow(agent) } }) { enabled -> act { app.setEnabled(agent, enabled) } } }
                agent.hook?.let { hook ->
                    item(key = "hook") {
                        HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                        WebhookSection(hook, secret = app.hookSecrets[agent.id]) { rotating = true }
                    }
                }
                item(key = "prompt") {
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Heading(stringResource(R.string.agent_prompt))
                        SelectionContainer { Text(agent.prompt, style = MaterialTheme.typography.bodyMedium) }
                    }
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                    Heading(stringResource(R.string.agent_runs), Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp))
                }
                when {
                    runs == null -> item(key = "loading") { Loading() }
                    runs.isEmpty() -> item(key = "none") {
                        Text(stringResource(R.string.no_runs_yet), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText,
                            modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp))
                    }
                    else -> items(runs, key = { "run-${it.id}" }) { run ->
                        val conversationId = run.conversationId
                        if (conversationId != null) {
                            val starred = app.isStarred(conversationId)
                            MenuRow(
                                selected = false,
                                onClick = { app.selection = conversationId },
                                menu = listOf(
                                    MenuAction(
                                        stringResource(if (starred) R.string.unstar else R.string.star),
                                        if (starred) Icons.Outlined.StarBorder else Icons.Outlined.Star,
                                    ) {
                                        act { app.setStarred(conversationId, !starred) }
                                    },
                                ),
                            ) { RunRow(run, starred) }
                        } else {
                            // It never started: nothing to open (or star).
                            Box(Modifier.padding(horizontal = 8.dp)) { RunRow(run, starred = false) }
                        }
                    }
                }
                item(key = "end") { Spacer(Modifier.size(24.dp)) }
            }
        }
    }

    if (deleting && agent != null) {
        DeleteAgentDialog(agent, onDismiss = { deleting = false }) {
            deleting = false
            act { app.deleteAgent(agent) }
        }
    }
    if (rotating && agent != null) {
        AlertDialog(
            onDismissRequest = { rotating = false },
            title = { Text(stringResource(R.string.hook_new_secret_title)) },
            text = { Text(stringResource(R.string.hook_new_secret_text)) },
            confirmButton = {
                TextButton(onClick = {
                    rotating = false
                    act { app.rotateHook(agent) }
                }) { Text(stringResource(R.string.hook_new_secret)) }
            },
            dismissButton = { TextButton(onClick = { rotating = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), actionError) { actionError = null }
}

@Composable
private fun Heading(text: String, modifier: Modifier = Modifier) {
    Text(text, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary, modifier = modifier)
}

/** What starts it, what it is doing right now, and the two things to do about that. */
@Composable
private fun Summary(agent: Agent, onRun: () -> Unit, onEnable: (Boolean) -> Unit) {
    val context = LocalContext.current
    val status = agent.status
    val small = MaterialTheme.typography.bodySmall
    Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            RowIcon(
                if (agent.enabled) triggerIcon(agent.trigger) else Icons.Outlined.Pause,
                tint = if (agent.enabled) MaterialTheme.colorScheme.primary else CedarColors.secondaryText,
                running = agent.lastRun?.isActive == true,
            )
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text(stringResource(triggerGroupLabel(agent.trigger.group)), style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText)
                Text(context.describe(agent.trigger), style = MaterialTheme.typography.bodyLarge)
            }
        }
        TriggerDetails(agent)
        if (status is Agent.Status.CheckFailing) {
            Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Icon(Icons.Outlined.WarningAmber, null, Modifier.size(16.dp), tint = CedarColors.red)
                Column {
                    Text(stringResource(R.string.agent_check_failing), style = small, fontWeight = FontWeight.SemiBold, color = CedarColors.red)
                    SelectionContainer { Text(status.error, style = small, color = CedarColors.red) }
                }
            }
        } else {
            Text(context.describe(status), style = small, color = CedarColors.secondaryText)
        }
        if (agent.trigger is Trigger.Unknown) {
            Text(stringResource(R.string.trigger_unknown_note), style = small, color = CedarColors.secondaryText)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            Button(onClick = onRun, enabled = agent.lastRun?.isActive != true) {
                Icon(Icons.Outlined.PlayCircle, null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.run_now))
            }
            OutlinedButton(onClick = { onEnable(!agent.enabled) }) {
                Icon(if (agent.enabled) Icons.Outlined.Pause else Icons.Outlined.PlayArrow, null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(stringResource(if (agent.enabled) R.string.pause else R.string.resume))
            }
        }
    }
}

/** What the one line leaves out: a watched address in full, a tool's input, how often it checks. */
@Composable
private fun TriggerDetails(agent: Agent) {
    val context = LocalContext.current
    val small = MaterialTheme.typography.bodySmall
    val trigger = agent.trigger
    if (trigger is Trigger.Condition) {
        when (val source = trigger.source) {
            is WatchSource.Url -> SelectionContainer { Text(source.url, style = small, fontFamily = FontFamily.Monospace, color = CedarColors.secondaryText) }
            is WatchSource.Tool -> if (source.input.isNotEmpty()) {
                SelectionContainer { Text(source.input.toString(), style = small, fontFamily = FontFamily.Monospace, color = CedarColors.secondaryText, maxLines = 6, overflow = TextOverflow.Ellipsis) }
            }
            is WatchSource.Records -> {}
        }
    }
    trigger.checkMinutes?.let {
        Text(stringResource(R.string.label_value, stringResource(R.string.agent_checks), context.everyText(it)), style = small, color = CedarColors.secondaryText)
    }
}

/** A webhook's address, its secret (in full only right after it was made), a call to try, and what the caller needs. */
@Composable
private fun WebhookSection(hook: Hook, secret: String?, onRotate: () -> Unit) {
    val small = MaterialTheme.typography.bodySmall
    Column(Modifier.fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Heading(stringResource(R.string.hook_title))
        Copyable(stringResource(R.string.hook_url), hook.url)
        if (secret != null) {
            Copyable(stringResource(R.string.hook_secret), secret)
            Text(stringResource(R.string.hook_secret_once), style = small, color = CedarColors.clay)
        } else {
            Text(
                stringResource(R.string.label_value, stringResource(R.string.hook_secret), stringResource(R.string.hook_secret_ends, hook.secretLast4.orEmpty())),
                style = MaterialTheme.typography.bodyMedium,
            )
            Text(stringResource(R.string.hook_secret_hidden), style = small, color = CedarColors.secondaryText)
        }
        OutlinedButton(onClick = onRotate) { Text(stringResource(R.string.hook_new_secret)) }
        Copyable(stringResource(R.string.hook_example), hook.curlExample(secret), scrolls = true)
        Text(stringResource(R.string.hook_reachable), style = small, color = CedarColors.secondaryText)
    }
}

/** A value in monospace with its label over it and a copy button beside it. */
@Composable
private fun Copyable(label: String, value: String, scrolls: Boolean = false) {
    val context = LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Text(label, style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText)
        Row(
            Modifier.fillMaxWidth().surfaceCard(radius = 10.dp).padding(start = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            SelectionContainer(Modifier.weight(1f).then(if (scrolls) Modifier.horizontalScroll(rememberScrollState()) else Modifier)) {
                Text(value, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, softWrap = !scrolls, modifier = Modifier.padding(vertical = 8.dp))
            }
            IconButton(onClick = { copyText(context, value) }) {
                Icon(Icons.Outlined.ContentCopy, stringResource(R.string.copy_what, label), Modifier.size(18.dp), tint = CedarColors.secondaryText)
            }
        }
    }
}

/** When a run started: "Mon 27 Oct, 07:30", as the user's locale orders it. */
private fun runDate(run: AgentRun): String = run.time?.let { skeletonDate(it, "EEEdMMMHHmm") }.orEmpty()

@Composable
private fun RunRow(run: AgentRun, starred: Boolean) {
    Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 8.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.Top) {
        UnreadDot(run.unread, Modifier.padding(top = 7.dp), size = 7)
        Spacer(Modifier.width(8.dp))
        Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(runDate(run), style = MaterialTheme.typography.bodyMedium,
                    fontWeight = if (run.unread) FontWeight.SemiBold else FontWeight.Normal)
                if (starred) StarMark()
                RunOutcomeLabel(run.outcome)
            }
            RunCauseLine(run)
            // The start of what came with it: the mail, the call's body, the value.
            run.cause?.excerpt?.takeIf { it.isNotBlank() }?.let {
                Text(it, style = MaterialTheme.typography.labelSmall, fontFamily = FontFamily.Monospace, color = CedarColors.secondaryText, maxLines = 2, overflow = TextOverflow.Ellipsis)
            }
            if (run.outcome == AgentRun.Outcome.NOT_STARTED) {
                run.error?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText, maxLines = 2, overflow = TextOverflow.Ellipsis) }
            }
        }
    }
}
