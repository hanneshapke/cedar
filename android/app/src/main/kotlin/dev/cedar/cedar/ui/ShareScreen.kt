package dev.cedar.cedar.ui

import androidx.compose.foundation.clickable
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.HourglassEmpty
import androidx.compose.material.icons.outlined.SmartToy
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.Button
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
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
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.Agent
import dev.cedar.core.DeviceRequest
import dev.cedar.core.Trigger
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/** Handing what was shared to an agent, and how that went. */
private sealed interface Handing {
    data object Idle : Handing
    data class Sending(val agent: Agent) : Handing

    /** A run started; its conversation can be opened. */
    data class Started(val agent: Agent, val conversationId: String?) : Handing

    /** The agent may not start right now. Nothing is queued: the user tries again, or picks another. */
    data class Held(val agent: Agent, val why: String?) : Handing
    data class Failed(val agent: Agent, val message: String) : Handing
}

/**
 * What the share sheet opens: what was shared, and the agents that are switched on, to pick the
 * one that gets it. The ones that wait for the device come first, because they were made for
 * this; any other can take it too. Then whether a run started (and a way to open it), or why not,
 * in words.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ShareScreen(app: AppModel, shared: DeviceRequest?, onOpenRun: (String) -> Unit, onOpenCedar: () -> Unit, onClose: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var handing by remember { mutableStateOf<Handing>(Handing.Idle) }
    // Before the language is picked and the server entered, there is nothing to hand anything to.
    val connected = app.language != null && app.settings.isConfigured

    LaunchedEffect(connected) { if (connected) app.refreshAgents() }

    fun hand(agent: Agent) {
        val client = app.client ?: return
        val request = shared ?: return
        handing = Handing.Sending(agent)
        scope.launch {
            handing = try {
                val start = client.triggerAgent(agent.id, request.source, request.text, request.title, request.url)
                if (start.started) Handing.Started(agent, start.conversationId) else Handing.Held(agent, start.held)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                // The server's own words for a refusal: "this agent is paused".
                Handing.Failed(agent, context.describe(e))
            }
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.share_target_label), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = { IconButton(onClick = onClose) { Icon(Icons.Filled.Close, stringResource(R.string.close)) } },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            val state = handing
            when {
                !connected -> Notice(stringResource(R.string.share_not_connected_title), stringResource(R.string.share_not_connected_text)) {
                    Button(onClick = onOpenCedar) { Text(stringResource(R.string.share_open_cedar)) }
                }
                shared == null -> Notice(stringResource(R.string.share_nothing_title), stringResource(R.string.share_nothing_text)) {
                    OutlinedButton(onClick = onClose) { Text(stringResource(R.string.close)) }
                }
                state is Handing.Started -> Column(Modifier.widthIn(max = 640.dp).fillMaxWidth().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    Outcome(Icons.Outlined.CheckCircle, CedarColors.green, stringResource(R.string.share_started, state.agent.title), null)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        state.conversationId?.let { id -> Button(onClick = { onOpenRun(id) }) { Text(stringResource(R.string.share_open_run)) } }
                        OutlinedButton(onClick = onClose) { Text(stringResource(R.string.done)) }
                    }
                }
                else -> {
                    val agents = app.agents.filter { it.enabled }
                    // In the server's order within each: the ones made for this, then the rest.
                    val forDevice = agents.filter { it.trigger is Trigger.Device }
                    val others = agents.filter { it.trigger !is Trigger.Device }
                    val busy = state is Handing.Sending
                    LazyColumn(Modifier.widthIn(max = 640.dp).fillMaxSize()) {
                        item(key = "shared") { SharedPreview(shared) }
                        when (state) {
                            is Handing.Held -> item(key = "outcome") {
                                Outcome(
                                    Icons.Outlined.HourglassEmpty, CedarColors.clay,
                                    stringResource(R.string.share_not_started, state.agent.title), context.describeHeld(state.why),
                                )
                            }
                            is Handing.Failed -> item(key = "outcome") {
                                Outcome(Icons.Outlined.WarningAmber, CedarColors.red, stringResource(R.string.share_not_started, state.agent.title), state.message)
                            }
                            else -> {}
                        }
                        app.agentError?.let { error -> item(key = "error") { ErrorLine(error) } }
                        if (agents.isEmpty()) {
                            if (app.agentsLoaded) {
                                item(key = "none") {
                                    EmptyState(
                                        Icons.Outlined.SmartToy, stringResource(R.string.share_no_agents_title), stringResource(R.string.share_no_agents_text),
                                        Modifier.padding(top = 24.dp),
                                    )
                                }
                            } else if (app.agentError == null) {
                                item(key = "loading") { Loading() }
                            }
                        } else {
                            item(key = "pick") {
                                Text(
                                    stringResource(R.string.share_pick), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary,
                                    modifier = Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 4.dp),
                                )
                            }
                            if (forDevice.isNotEmpty()) {
                                item(key = "group-device") { GroupLabel(stringResource(R.string.share_group_device)) }
                                items(forDevice, key = { "agent-${it.id}" }) { agent -> ShareRow(agent, sending = (state as? Handing.Sending)?.agent?.id == agent.id, enabled = !busy) { hand(agent) } }
                            }
                            if (others.isNotEmpty()) {
                                item(key = "group-others") { GroupLabel(stringResource(if (forDevice.isEmpty()) R.string.share_group_all else R.string.share_group_others)) }
                                items(others, key = { "agent-${it.id}" }) { agent -> ShareRow(agent, sending = (state as? Handing.Sending)?.agent?.id == agent.id, enabled = !busy) { hand(agent) } }
                            }
                        }
                        item(key = "end") { Spacer(Modifier.size(24.dp)) }
                    }
                }
            }
        }
    }
}

/** A message in place of the list, with the one thing to do about it. */
@Composable
private fun Notice(title: String, text: String, action: @Composable () -> Unit) {
    Column(
        Modifier.widthIn(max = 480.dp).fillMaxWidth().padding(32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        CedarBadge()
        Text(title, style = MaterialTheme.typography.titleMedium)
        Text(text, style = MaterialTheme.typography.bodyMedium, color = CedarColors.secondaryText)
        action()
    }
}

@Composable
private fun GroupLabel(title: String) {
    Text(
        title,
        style = MaterialTheme.typography.labelMedium,
        fontWeight = FontWeight.SemiBold,
        color = CedarColors.secondaryText,
        modifier = Modifier.padding(start = 24.dp, end = 16.dp, top = 12.dp, bottom = 4.dp),
    )
}

/** What the agent will be handed: the title and the link, if the other app gave them, and the start of the text. */
@Composable
private fun SharedPreview(shared: DeviceRequest) {
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).surfaceCard(radius = 12.dp).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(4.dp),
    ) {
        shared.title?.let { Text(it, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis) }
        shared.url?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary, maxLines = 2, overflow = TextOverflow.Ellipsis) }
        shared.text?.let { Text(it, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 4, overflow = TextOverflow.Ellipsis) }
    }
}

/** How handing it over went: a line, and under it the reason when it didn't start. */
@Composable
private fun Outcome(icon: ImageVector, color: Color, title: String, detail: String?) {
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)
            .surfaceCard(radius = 12.dp, fill = color.copy(alpha = 0.08f), stroke = color.copy(alpha = 0.3f)).padding(12.dp),
        verticalAlignment = Alignment.Top,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Icon(icon, null, Modifier.size(18.dp), tint = color)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
            detail?.let { Text(it, style = MaterialTheme.typography.bodySmall) }
        }
    }
}

/** An agent to hand it to: its name and what starts it otherwise. */
@Composable
private fun ShareRow(agent: Agent, sending: Boolean, enabled: Boolean, onClick: () -> Unit) {
    val context = LocalContext.current
    Row(
        Modifier
            .fillMaxWidth()
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(14.dp))
            .clickable(enabled = enabled, onClick = onClick)
            .alpha(if (enabled || sending) 1f else 0.5f)
            .padding(horizontal = 8.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RowIcon(triggerIcon(agent.trigger), running = sending)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(agent.title, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(context.describe(agent.trigger), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}
