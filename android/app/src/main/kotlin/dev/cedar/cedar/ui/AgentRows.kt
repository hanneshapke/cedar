package dev.cedar.cedar.ui

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
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.background
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Email
import androidx.compose.material.icons.outlined.Event
import androidx.compose.material.icons.outlined.Extension
import androidx.compose.material.icons.outlined.HelpOutline
import androidx.compose.material.icons.outlined.History
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.Pause
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.SmartToy
import androidx.compose.material.icons.outlined.Star
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material.icons.outlined.Webhook
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AgentFormRequest
import dev.cedar.cedar.AgentsTab
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.Agent
import dev.cedar.core.AgentRun
import dev.cedar.core.Trigger
import dev.cedar.core.TriggerGroup
import dev.cedar.core.TriggerKind
import kotlinx.coroutines.launch

/**
 * The Agents list, as the web chat's Agents page has it: an Agents | Activity switch, then either
 * every agent under what starts it (a schedule, a change, a webhook, mail and the calendar), each
 * with its trigger in a line, what it is doing and how its last run went; or the latest runs of
 * all of them. Tapping an agent opens its page; a long press offers Run now, Pause or Resume,
 * Edit and Delete. Tapping a run opens it as a conversation.
 */
@Composable
fun AgentRows(app: AppModel, onAction: (suspend () -> Unit) -> Unit) {
    LaunchedEffect(app.agentsTab) { if (app.agentsTab == AgentsTab.ACTIVITY) app.refreshActivity() }
    Column(Modifier.fillMaxSize()) {
        Row(
            Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, bottom = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            SegmentedSwitch(
                options = listOf(AgentsTab.AGENTS to stringResource(R.string.tab_agents), AgentsTab.ACTIVITY to stringResource(R.string.agents_activity)),
                selected = app.agentsTab,
                onSelect = { app.agentsTab = it },
                modifier = Modifier.weight(1f),
            )
            OutlinedButton(onClick = { app.agentForm = AgentFormRequest() }, enabled = app.settings.isConfigured) {
                Icon(Icons.Outlined.Add, null, Modifier.size(18.dp))
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.new_agent), maxLines = 1)
            }
        }
        when (app.agentsTab) {
            AgentsTab.AGENTS -> AgentList(app, onAction)
            AgentsTab.ACTIVITY -> ActivityList(app, onAction)
        }
    }
}

private val GROUPS = listOf(TriggerGroup.SCHEDULE, TriggerGroup.CONDITION, TriggerGroup.WEBHOOK, TriggerGroup.MAIL_CALENDAR, TriggerGroup.OTHER)

@Composable
private fun AgentList(app: AppModel, onAction: (suspend () -> Unit) -> Unit) {
    var deleting by remember { mutableStateOf<Agent?>(null) }
    if (app.agents.isEmpty() && app.agentError == null) {
        // In a scrollable column, so pulling to refresh still works on an empty list.
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                if (app.agentsLoaded) {
                    EmptyState(
                        Icons.Outlined.SmartToy, stringResource(R.string.empty_agents_title), stringResource(R.string.empty_agents_text),
                        Modifier.padding(top = 56.dp),
                    )
                } else {
                    Loading()
                }
            }
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        app.agentError?.let { error -> item { ErrorLine(error) } }
        for (group in GROUPS) {
            // In the server's order: the ones switched on first.
            val agents = app.agents.filter { it.trigger.group == group }
            if (agents.isEmpty()) continue
            item(key = "group-$group") { GroupHeader(stringResource(triggerGroupLabel(group))) }
            items(agents, key = { "agent-${it.id}" }) { agent ->
                MenuRow(
                    selected = app.showingAgent == agent.id && app.selection == null,
                    onClick = { app.openAgent(agent.id) },
                    menu = listOf(
                        MenuAction(stringResource(R.string.run_now), Icons.Outlined.PlayCircle, enabled = agent.lastRun?.isActive != true) {
                            onAction { app.runNow(agent) }
                        },
                        if (agent.enabled) {
                            MenuAction(stringResource(R.string.pause), Icons.Outlined.Pause) { onAction { app.setEnabled(agent, false) } }
                        } else {
                            MenuAction(stringResource(R.string.resume), Icons.Outlined.PlayArrow) { onAction { app.setEnabled(agent, true) } }
                        },
                        MenuAction(stringResource(R.string.edit), Icons.Outlined.Edit) { app.agentForm = AgentFormRequest(agent = agent) },
                        MenuAction(stringResource(R.string.delete), Icons.Outlined.Delete, destructive = true) { deleting = agent },
                    ),
                ) {
                    AgentRow(agent, unread = agent.id in app.unreadAgents)
                }
            }
        }
    }
    deleting?.let { agent ->
        DeleteAgentDialog(agent, onDismiss = { deleting = null }) {
            deleting = null
            onAction { app.deleteAgent(agent) }
        }
    }
}

@Composable
private fun GroupHeader(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.labelMedium,
        fontWeight = FontWeight.SemiBold,
        color = CedarColors.secondaryText,
        modifier = Modifier.padding(start = 24.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
    )
}

/** The picture of what starts an agent. */
fun triggerIcon(trigger: Trigger): ImageVector = when (trigger) {
    is Trigger.Time, is Trigger.Once, is Trigger.Interval, is Trigger.Monthly -> Icons.Outlined.Schedule
    is Trigger.Condition -> Icons.Outlined.Visibility
    Trigger.Webhook -> Icons.Outlined.Webhook
    is Trigger.Email -> Icons.Outlined.Email
    is Trigger.Calendar -> Icons.Outlined.Event
    is Trigger.Unknown -> triggerKindIcon(trigger.kind)
}

/** The same from a trigger's `kind` alone, as the activity feed sends it. */
fun triggerKindIcon(kind: String): ImageVector = when (TriggerKind.of(kind)) {
    TriggerKind.TIME, TriggerKind.ONCE, TriggerKind.INTERVAL, TriggerKind.MONTHLY -> Icons.Outlined.Schedule
    TriggerKind.CONDITION -> Icons.Outlined.Visibility
    TriggerKind.WEBHOOK -> Icons.Outlined.Webhook
    TriggerKind.EMAIL -> Icons.Outlined.Email
    TriggerKind.CALENDAR -> Icons.Outlined.Event
    null -> Icons.Outlined.Extension
}

@Composable
private fun AgentRow(agent: Agent, unread: Boolean) {
    val context = LocalContext.current
    val status = agent.status
    val failing = status is Agent.Status.CheckFailing
    Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        RowIcon(
            if (agent.enabled) triggerIcon(agent.trigger) else Icons.Outlined.Pause,
            tint = if (agent.enabled) MaterialTheme.colorScheme.primary else CedarColors.secondaryText,
            running = agent.lastRun?.isActive == true,
        )
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f).alpha(if (agent.enabled) 1f else 0.7f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(agent.title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal, modifier = Modifier.weight(1f, fill = false))
                if (unread) UnreadDot(true, size = 9)
            }
            val small = MaterialTheme.typography.bodySmall
            Text(context.describe(agent.trigger), style = small, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                if (failing) Icon(Icons.Outlined.WarningAmber, null, Modifier.size(12.dp), tint = CedarColors.red)
                Text(
                    context.describe(status), style = small, color = if (failing) CedarColors.red else CedarColors.secondaryText,
                    maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false),
                )
                agent.lastRun?.let {
                    Text("·", style = small, color = CedarColors.secondaryText)
                    RunOutcomeLabel(it.outcome)
                }
            }
        }
    }
}

/** Every agent's latest runs, newest first: whose it was, what started it and how it went. */
@Composable
private fun ActivityList(app: AppModel, onAction: (suspend () -> Unit) -> Unit) {
    val scope = rememberCoroutineScope()
    if (app.activity.isEmpty() && app.activityError == null) {
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                if (app.activityLoaded) {
                    EmptyState(
                        Icons.Outlined.History, stringResource(R.string.empty_activity_title), stringResource(R.string.empty_activity_text),
                        Modifier.padding(top = 56.dp),
                    )
                } else {
                    Loading()
                }
            }
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        app.activityError?.let { error -> item { ErrorLine(error) } }
        items(app.activity, key = { "activity-${it.id}" }) { run ->
            val conversationId = run.conversationId
            val agentId = run.agent?.id ?: run.agentId
            val openAgent = MenuAction(stringResource(R.string.open_agent), Icons.Outlined.SmartToy) { app.openAgent(agentId) }
            if (conversationId != null) {
                val starred = app.isStarred(conversationId)
                MenuRow(
                    selected = app.selection == conversationId,
                    onClick = { app.selection = conversationId },
                    menu = listOf(
                        MenuAction(
                            stringResource(if (starred) R.string.unstar else R.string.star),
                            if (starred) Icons.Outlined.StarBorder else Icons.Outlined.Star,
                        ) {
                            onAction { app.setStarred(conversationId, !starred) }
                        },
                        openAgent,
                    ),
                ) { ActivityRow(run, starred) }
            } else {
                // It never started: there is no conversation to open, only its agent.
                MenuRow(selected = false, onClick = { app.openAgent(agentId) }, menu = listOf(openAgent)) { ActivityRow(run, starred = false) }
            }
        }
        if (app.activityNext != null) {
            item(key = "more") {
                Box(Modifier.fillMaxWidth().padding(12.dp), contentAlignment = Alignment.Center) {
                    if (app.activityLoadingMore) {
                        CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
                    } else {
                        TextButton(onClick = { scope.launch { app.loadMoreActivity() } }) { Text(stringResource(R.string.show_more)) }
                    }
                }
            }
        }
    }
}

@Composable
private fun ActivityRow(run: AgentRun, starred: Boolean) {
    val title = run.agent?.title?.takeIf { it.isNotBlank() } ?: stringResource(R.string.agent)
    Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        RowIcon(triggerKindIcon(run.agent?.triggerKind.orEmpty()), running = run.isActive)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            val small = MaterialTheme.typography.bodySmall
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        fontWeight = if (run.unread) FontWeight.SemiBold else FontWeight.Normal, modifier = Modifier.weight(1f, fill = false))
                    if (starred) StarMark()
                    if (run.unread) UnreadDot(true, size = 9)
                }
                run.time?.let { Text(relative(it), style = small, color = CedarColors.secondaryText, maxLines = 1) }
            }
            RunCauseLine(run)
            RunOutcomeLabel(run.outcome)
            if (run.outcome == AgentRun.Outcome.NOT_STARTED) {
                run.error?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText, maxLines = 2, overflow = TextOverflow.Ellipsis) }
            }
        }
    }
}

/** What started a run: the server's line for it, or the Run now chip; nothing for a run from before agents said. */
@Composable
fun RunCauseLine(run: AgentRun, modifier: Modifier = Modifier) {
    val startedBy = run.startedBy
    when {
        run.manual -> Text(
            stringResource(R.string.run_now),
            style = MaterialTheme.typography.labelSmall,
            fontWeight = FontWeight.SemiBold,
            color = CedarColors.secondaryText,
            modifier = modifier
                .clip(RoundedCornerShape(50))
                .background(MaterialTheme.colorScheme.surfaceContainerHighest)
                .padding(horizontal = 8.dp, vertical = 2.dp),
        )
        startedBy != null -> Text(
            stringResource(R.string.started_by, startedBy),
            style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 2, overflow = TextOverflow.Ellipsis,
            modifier = modifier,
        )
    }
}

@Composable
fun RunOutcomeLabel(outcome: AgentRun.Outcome) {
    val secondary = CedarColors.secondaryText
    val (icon, label, color) = when (outcome) {
        AgentRun.Outcome.QUEUED -> Triple(Icons.Outlined.HourglassEmpty, R.string.outcome_queued, secondary)
        AgentRun.Outcome.RUNNING -> Triple(Icons.Outlined.Sync, R.string.outcome_running, secondary)
        AgentRun.Outcome.COMPLETED -> Triple(Icons.Outlined.CheckCircle, R.string.outcome_done, secondary)
        AgentRun.Outcome.NEEDS_ANSWER -> Triple(Icons.Outlined.HelpOutline, R.string.needs_answer, CedarColors.clay)
        AgentRun.Outcome.FAILED -> Triple(Icons.Outlined.Cancel, R.string.failed, CedarColors.red)
        AgentRun.Outcome.CANCELLED -> Triple(Icons.Outlined.StopCircle, R.string.outcome_stopped, secondary)
        AgentRun.Outcome.NOT_STARTED -> Triple(Icons.Outlined.WarningAmber, R.string.outcome_not_started, CedarColors.red)
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        Icon(icon, null, Modifier.size(12.dp), tint = color)
        Text(stringResource(label), style = MaterialTheme.typography.bodySmall, color = color, maxLines = 1)
    }
}

/** Asks before deleting an agent: its runs and their conversations go with it. */
@Composable
fun DeleteAgentDialog(agent: Agent, onDismiss: () -> Unit, onDelete: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.delete_agent_title, agent.title)) },
        text = { Text(stringResource(R.string.delete_agent_text)) },
        confirmButton = { TextButton(onClick = onDelete) { Text(stringResource(R.string.delete), color = CedarColors.red) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}
