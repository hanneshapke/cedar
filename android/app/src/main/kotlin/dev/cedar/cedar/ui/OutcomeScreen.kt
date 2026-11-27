package dev.cedar.cedar.ui

import androidx.compose.animation.animateContentSize
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Button
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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.ChatException
import dev.cedar.core.Followup
import dev.cedar.core.FollowupChoice
import dev.cedar.core.Outcome
import dev.cedar.core.TurnState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.util.TimeZone

/** How often an outcome whose response is still going is read again. */
private const val WORKING_REFRESH_MILLIS = 3_000L

/**
 * What one ask came to, as the web's outcome page: what was asked, the answer headed by its gist,
 * then what it left behind (what it held for approval, the files it wrote or changed, what it
 * remembered, the record rows it saved, the follow-up it suggested) and, folded, how Cedar got
 * there (its steps and sources). Keep going sends the next message from here and opens the
 * conversation, where its response streams; a response still going is followed until it ends.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun OutcomeScreen(app: AppModel, turnId: String) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var outcome by remember { mutableStateOf<Outcome?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var actionError by remember { mutableStateOf<String?>(null) }
    // What was decided or forgotten here, since the outcome was read.
    var forgotten by remember { mutableStateOf<Set<String>>(emptySet()) }
    var decisions by remember { mutableStateOf<Map<String, Followup>>(emptyMap()) }
    var deciding by remember { mutableStateOf<Set<String>>(emptySet()) }
    var followupErrors by remember { mutableStateOf<Map<String, String>>(emptyMap()) }
    var showingFile by remember { mutableStateOf<String?>(null) }
    var howOpen by rememberSaveable { mutableStateOf(false) }
    var next by rememberSaveable { mutableStateOf("") }
    var sendingNext by remember { mutableStateOf(false) }

    LaunchedEffect(turnId) {
        while (true) {
            val client = app.client
            if (client == null) {
                error = app.describe(ChatException.NotConfigured)
                break
            }
            try {
                val read = client.outcome(turnId)
                outcome = read
                error = null
                if (read.state != TurnState.WORKING) break
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = app.describe(e)
                break
            }
            delay(WORKING_REFRESH_MILLIS)
        }
    }

    val shown = outcome
    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(shown?.conversation?.title?.takeIf { it.isNotBlank() } ?: stringResource(R.string.tab_today), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = {
                    IconButton(onClick = { app.showingOutcome = null }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 640.dp).fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(14.dp),
            ) {
                error?.let { ErrorLine(it) }
                if (shown == null) {
                    if (error == null) Loading()
                    return@Column
                }
                val turn = shown.turn
                val small = MaterialTheme.typography.bodySmall
                val secondary = CedarColors.secondaryText

                // How it stands, what was asked, and what it came to.
                val time = shortTime(turn.finishedAt ?: turn.createdAt)
                val standing = when (shown.state) {
                    TurnState.DONE -> stringResource(R.string.today_outcome_done_at, time)
                    TurnState.FAILED -> stringResource(R.string.today_outcome_failed_at, time)
                    TurnState.STOPPED -> stringResource(R.string.today_stopped)
                    TurnState.ASKING -> stringResource(R.string.today_pill_asking)
                    TurnState.WAITING -> stringResource(R.string.today_pill_waiting)
                    else -> stringResource(R.string.today_working)
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (shown.state == TurnState.WORKING) CircularProgressIndicator(Modifier.size(14.dp), strokeWidth = 2.dp)
                    Text(
                        listOfNotNull(standing, shown.agent?.title?.takeIf { it.isNotEmpty() }).joinToString(" · "),
                        style = small,
                        color = if (shown.state == TurnState.FAILED) CedarColors.red else secondary,
                    )
                }
                SelectionContainer {
                    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(stringResource(R.string.today_outcome_you_asked, turn.prompt.trim()), style = MaterialTheme.typography.bodyMedium, color = secondary)
                        Text(shown.headline, style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold)
                    }
                }
                if (shown.laterTurns > 0) {
                    Text(plural(R.plurals.today_outcome_continued, shown.laterTurns, shown.laterTurns), style = small, color = secondary)
                }
                // The answer, without the paragraph the headline already says.
                shown.answer?.let { MarkdownView(it) }

                // What it held for the person's OK: what still waits first.
                val held = shown.heldOpenFirst.map { app.heldCall(it.id) ?: it }
                if (held.isNotEmpty()) {
                    OutcomeHeading(stringResource(if (held.any { it.isOpen }) R.string.today_outcome_held else R.string.today_outcome_held_done))
                    held.forEach { item ->
                        HeldCallView(
                            note = null,
                            item = item,
                            gone = false,
                            deciding = app.isDecidingHeldCall(item.id),
                            error = app.heldCallError(item.id),
                            onDecide = { approve -> app.decideHeldCall(item.id, approve) },
                        )
                    }
                }

                if (shown.files.isNotEmpty()) {
                    OutcomeHeading(stringResource(R.string.files))
                    shown.files.forEach { file ->
                        Row(
                            Modifier.fillMaxWidth().surfaceCard(radius = 12.dp).clickable { showingFile = file.path }.padding(12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(12.dp),
                        ) {
                            Icon(fileIcon(file.path), null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
                            Column(Modifier.weight(1f)) {
                                Text(file.path.substringAfterLast('/'), fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                Text("${file.path} · ${context.fileSize(file.size)}", style = small, color = secondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }

                if (shown.memories.isNotEmpty()) {
                    OutcomeHeading(stringResource(R.string.today_outcome_remembered))
                    shown.memories.forEach { memory ->
                        val gone = memory.id in forgotten
                        Row(
                            Modifier.fillMaxWidth().surfaceCard(radius = 12.dp).padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                            horizontalArrangement = Arrangement.spacedBy(8.dp),
                        ) {
                            Text(
                                if (gone) stringResource(R.string.today_outcome_forgotten, memory.text) else memory.text,
                                Modifier.weight(1f).padding(vertical = 8.dp),
                                color = if (gone) secondary else MaterialTheme.colorScheme.onSurface,
                            )
                            if (!gone) {
                                TextButton(onClick = {
                                    scope.launch {
                                        try {
                                            val client = app.client ?: throw ChatException.NotConfigured
                                            client.deleteMemory(memory.id)
                                            forgotten = forgotten + memory.id
                                        } catch (e: CancellationException) {
                                            throw e
                                        } catch (e: Exception) {
                                            actionError = app.describe(e)
                                        }
                                    }
                                }) { Text(stringResource(R.string.today_outcome_forget)) }
                            }
                        }
                    }
                }

                if (shown.records.isNotEmpty()) {
                    OutcomeHeading(stringResource(R.string.today_outcome_records))
                    shown.records.forEach { record ->
                        Column(Modifier.fillMaxWidth().surfaceCard(radius = 12.dp).padding(12.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                            Text(record.label, fontWeight = FontWeight.Medium)
                            record.fields.forEach { (name, value) ->
                                Text("$name: $value", style = small, color = secondary, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }

                // What it suggested to do later: nothing is scheduled until the person says so.
                shown.followups.forEach { followup ->
                    val suggestion = remember(followup.output) { followup.suggestion }
                    if (suggestion != null) {
                        val key = followup.callId
                        FollowUpView(
                            suggestion,
                            decision = decisions[key] ?: shown.decision(key),
                            deciding = key in deciding,
                            error = followupErrors[key],
                            onDecide = { accept ->
                                if (key !in deciding) {
                                    deciding = deciding + key
                                    followupErrors = followupErrors - key
                                    scope.launch {
                                        try {
                                            val client = app.client ?: throw ChatException.NotConfigured
                                            val choice = if (accept) {
                                                FollowupChoice.Accept(suggestion.title, suggestion.prompt, suggestion.atText, TimeZone.getDefault().id)
                                            } else {
                                                FollowupChoice.Dismiss
                                            }
                                            val decided = client.decideFollowup(turn.id, key, choice)
                                            decisions = decisions + (key to decided.followup)
                                            decided.agent?.let { app.put(it) }
                                        } catch (e: CancellationException) {
                                            throw e
                                        } catch (e: Exception) {
                                            followupErrors = followupErrors + (key to app.describe(e))
                                        } finally {
                                            deciding = deciding - key
                                        }
                                    }
                                }
                            },
                            onOpenAgent = { app.openAgent(it) },
                        )
                    }
                }

                // How it got there, folded.
                if (shown.steps.isNotEmpty() || shown.sources.isNotEmpty()) {
                    Column(Modifier.fillMaxWidth().surfaceCard(radius = 12.dp).animateContentSize()) {
                        val failed = shown.failedSteps
                        val counts = listOfNotNull(
                            stringResource(R.string.today_outcome_how),
                            plural(R.plurals.today_outcome_steps, shown.steps.size, shown.steps.size),
                            if (shown.sources.isNotEmpty()) plural(R.plurals.today_outcome_sources, shown.sources.size, shown.sources.size) else null,
                            if (failed > 0) plural(R.plurals.today_outcome_steps_failed, failed, failed) else null,
                        )
                        Row(
                            Modifier.fillMaxWidth().clickable { howOpen = !howOpen }.padding(12.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Text(
                                counts.joinToString(" · "),
                                Modifier.weight(1f),
                                style = MaterialTheme.typography.bodyMedium,
                                color = secondary,
                            )
                            Icon(
                                Icons.Outlined.ExpandMore, stringResource(if (howOpen) R.string.show_less else R.string.show_more),
                                Modifier.size(18.dp).rotate(if (howOpen) 180f else 0f), tint = secondary,
                            )
                        }
                        if (howOpen) {
                            Column(Modifier.padding(start = 12.dp, end = 12.dp, bottom = 12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                shown.steps.forEachIndexed { index, step ->
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text("${index + 1}. ${toolLabel(step.name)}", style = small)
                                        if (step.isError) {
                                            Spacer(Modifier.width(8.dp))
                                            Text(stringResource(R.string.today_outcome_step_failed), style = small, color = CedarColors.red)
                                        }
                                    }
                                }
                                shown.sources.forEach { source ->
                                    val link = source.link
                                    Text(
                                        source.label,
                                        if (link != null) Modifier.clickable { openUrl(context, link) } else Modifier,
                                        style = small,
                                        color = if (link != null) MaterialTheme.colorScheme.primary else secondary,
                                        maxLines = 2,
                                        overflow = TextOverflow.Ellipsis,
                                    )
                                }
                            }
                        }
                    }
                }

                // The next message is typed here and sent to the conversation, which opens: its
                // response streams there. Not while this one is still going.
                val conversationId = shown.conversation.id
                val asking = shown.state == TurnState.ASKING
                if (shown.state != TurnState.WORKING && shown.state != TurnState.WAITING) {
                    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedTextField(
                            value = next,
                            onValueChange = { next = it.take(32_000) },
                            placeholder = { Text(stringResource(if (asking) R.string.today_outcome_your_answer else R.string.today_outcome_keep_going_hint)) },
                            maxLines = 4,
                            enabled = !sendingNext,
                            modifier = Modifier.weight(1f),
                        )
                        Button(
                            onClick = {
                                val text = next.trim()
                                sendingNext = true
                                // The model's scope: this screen is left when the chat opens.
                                app.scope.launch {
                                    try {
                                        if (app.keepGoing(conversationId, text)) next = ""
                                        else actionError = app.chat(conversationId).errorMessage ?: app.describe(ChatException.NotConfigured)
                                    } catch (e: CancellationException) {
                                        throw e
                                    } catch (e: Exception) {
                                        actionError = app.describe(e)
                                    } finally {
                                        sendingNext = false
                                    }
                                }
                            },
                            enabled = next.isNotBlank() && !sendingNext,
                        ) { Text(stringResource(if (asking) R.string.answer else R.string.send)) }
                    }
                }
                OutlinedButton(onClick = { app.open(conversationId) }, modifier = Modifier.fillMaxWidth()) {
                    Text(stringResource(R.string.today_outcome_open))
                }
            }
        }
    }

    showingFile?.let { path ->
        val client = app.client
        val conversationId = shown?.conversation?.id
        if (client != null && conversationId != null) WorkspaceFileDialog(client, conversationId, path) { showingFile = null }
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), actionError) { actionError = null }
}

@Composable
private fun OutcomeHeading(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelMedium,
        fontWeight = FontWeight.SemiBold,
        color = CedarColors.secondaryText,
        modifier = Modifier.padding(top = 4.dp),
    )
}
