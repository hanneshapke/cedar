package dev.cedar.cedar.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Notes
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ExpandLess
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.HelpOutline
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.Pause
import androidx.compose.material.icons.outlined.PauseCircle
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material.icons.outlined.Star
import androidx.compose.material.icons.outlined.StopCircle
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.core.Schedule
import dev.cedar.core.ScheduleRun
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/**
 * The Scheduled list: each scheduled agent with its timing and last run, expanding to its runs, as
 * iOS/Cedar/Features/Schedules/ScheduleRows.swift. A run opens as a conversation, as a chat does.
 */
@Composable
fun ScheduleRows(app: AppModel, onAction: (suspend () -> Unit) -> Unit) {
    var showingPrompt by remember { mutableStateOf<Schedule?>(null) }
    if (app.schedules.isEmpty() && app.scheduleError == null) {
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                if (app.schedulesLoaded) {
                    EmptyState(
                        Icons.Outlined.Schedule, "No Scheduled Agents",
                        "On the web, the Schedule button turns a finished conversation into an agent that runs on its own.",
                        Modifier.padding(top = 80.dp),
                    )
                } else {
                    Loading()
                }
            }
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        app.scheduleError?.let { error -> item { ErrorLine(error) } }
        for (schedule in app.schedules) {
            val expanded = schedule.id in app.expandedSchedules
            item(key = "schedule-${schedule.id}") {
                MenuRow(
                    selected = false,
                    onClick = { app.toggleExpanded(schedule) },
                    menu = listOf(
                        MenuAction("Run Now", Icons.Outlined.PlayCircle, enabled = schedule.lastRun?.isActive != true) {
                            onAction { app.runNow(schedule) }
                        },
                        if (schedule.enabled) {
                            MenuAction("Pause", Icons.Outlined.Pause) { onAction { app.setEnabled(schedule, false) } }
                        } else {
                            MenuAction("Resume", Icons.Outlined.PlayArrow) { onAction { app.setEnabled(schedule, true) } }
                        },
                        MenuAction("Show Prompt", Icons.AutoMirrored.Outlined.Notes) { showingPrompt = schedule },
                    ),
                ) {
                    ScheduleRow(schedule, unread = schedule.id in app.unreadSchedules, expanded = expanded)
                }
            }
            if (expanded) {
                val runs = app.scheduleRuns[schedule.id]
                when {
                    runs == null -> item(key = "loading-${schedule.id}") { Loading() }
                    runs.isEmpty() -> item(key = "none-${schedule.id}") {
                        Text("No runs yet", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText,
                            modifier = Modifier.padding(start = 48.dp, top = 4.dp, bottom = 8.dp))
                    }
                    else -> items(runs, key = { "run-${it.id}" }) { run ->
                        val conversationId = run.conversationId
                        if (conversationId != null) {
                            val starred = app.isStarred(conversationId)
                            MenuRow(
                                selected = app.selection == conversationId,
                                onClick = { app.selection = conversationId },
                                menu = listOf(
                                    MenuAction(if (starred) "Unstar" else "Star", if (starred) Icons.Outlined.StarBorder else Icons.Outlined.Star) {
                                        onAction { app.setStarred(conversationId, !starred) }
                                    },
                                ),
                            ) { RunRow(run, starred) }
                        } else {
                            // It never started: nothing to open (or star).
                            RunRow(run, starred = false)
                        }
                    }
                }
            }
        }
    }
    showingPrompt?.let { schedule ->
        AlertDialog(
            onDismissRequest = { showingPrompt = null },
            title = { Text(schedule.title) },
            text = {
                Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) {
                    Text(schedule.cadence(), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                    Spacer(Modifier.size(12.dp))
                    SelectionContainer { Text(schedule.prompt) }
                }
            },
            confirmButton = { TextButton(onClick = { showingPrompt = null }) { Text("Done") } },
        )
    }
}

@Composable
private fun ScheduleRow(schedule: Schedule, unread: Boolean, expanded: Boolean) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.Top) {
        UnreadDot(unread, Modifier.padding(top = 7.dp))
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f).alpha(if (schedule.enabled) 1f else 0.7f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(schedule.title, maxLines = 1, overflow = TextOverflow.Ellipsis,
                    fontWeight = if (unread) FontWeight.SemiBold else FontWeight.Normal, modifier = Modifier.weight(1f, fill = false))
                if (schedule.lastRun?.isActive == true) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
            }
            val small = MaterialTheme.typography.bodySmall
            Text(schedule.cadence(), style = small, color = CedarColors.secondaryText, maxLines = 1)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                if (schedule.enabled) {
                    Text("Next ${relative(schedule.nextRunAt)}", style = small, color = CedarColors.secondaryText, maxLines = 1)
                } else {
                    Icon(Icons.Outlined.PauseCircle, null, Modifier.size(12.dp), tint = CedarColors.secondaryText)
                    Text("Paused", style = small, color = CedarColors.secondaryText)
                }
                schedule.lastRun?.let {
                    Text("·", style = small, color = CedarColors.secondaryText)
                    RunOutcomeLabel(it.outcome)
                }
            }
        }
        Icon(if (expanded) Icons.Outlined.ExpandLess else Icons.Outlined.ExpandMore, if (expanded) "Collapse" else "Expand",
            tint = CedarColors.secondaryText)
    }
}

private val runDate = DateTimeFormatter.ofPattern("EEE d MMM, HH:mm").withZone(ZoneId.systemDefault())

@Composable
private fun RunRow(run: ScheduleRun, starred: Boolean) {
    Row(Modifier.fillMaxWidth().padding(start = 40.dp, end = 16.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.Top) {
        UnreadDot(run.unread, Modifier.padding(top = 7.dp), size = 6)
        Spacer(Modifier.width(8.dp))
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(runDate.format(run.dueAt), style = MaterialTheme.typography.bodyMedium,
                    fontWeight = if (run.unread) FontWeight.SemiBold else FontWeight.Normal)
                if (starred) StarMark()
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                RunOutcomeLabel(run.outcome)
                if (run.manual) Text("· Run now", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            }
            if (run.outcome == ScheduleRun.Outcome.NOT_STARTED) {
                run.error?.let { Text(it, style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText, maxLines = 2) }
            }
        }
    }
}

@Composable
private fun RunOutcomeLabel(outcome: ScheduleRun.Outcome) {
    val secondary = CedarColors.secondaryText
    val (icon, label, color) = when (outcome) {
        ScheduleRun.Outcome.QUEUED -> Triple(Icons.Outlined.HourglassEmpty, "Queued", secondary)
        ScheduleRun.Outcome.RUNNING -> Triple(Icons.Outlined.Sync, "Running", secondary)
        ScheduleRun.Outcome.COMPLETED -> Triple(Icons.Outlined.CheckCircle, "Done", secondary)
        ScheduleRun.Outcome.NEEDS_ANSWER -> Triple(Icons.Outlined.HelpOutline, "Needs answer", CedarColors.orange)
        ScheduleRun.Outcome.FAILED -> Triple(Icons.Outlined.Cancel, "Failed", CedarColors.red)
        ScheduleRun.Outcome.CANCELLED -> Triple(Icons.Outlined.StopCircle, "Stopped", secondary)
        ScheduleRun.Outcome.NOT_STARTED -> Triple(Icons.Outlined.WarningAmber, "Didn't start", CedarColors.red)
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(3.dp)) {
        Icon(icon, null, Modifier.size(12.dp), tint = color)
        Text(label, style = MaterialTheme.typography.bodySmall, color = color, maxLines = 1)
    }
}
