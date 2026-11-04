package dev.cedar.cedar.ui

import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.ThumbDown
import androidx.compose.material.icons.outlined.ThumbUp
import androidx.compose.material3.Checkbox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.ContributionPreview
import dev.cedar.core.bool
import dev.cedar.core.get
import dev.cedar.core.int
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Contribute this conversation: the review the web chat's dialog shows, in native form. The
 * conversation as it would leave the server (prompts, the agent's text and tool calls, with the
 * stand-ins the redaction wrote listed), why it can't go if it can't, what to include, a thumb
 * per response, the outcome, and Send. What is sent is the bundle the server previewed, with the
 * outcome and ratings put in (`ContributionPreview.bundleWith`).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ContributeDialog(app: AppModel, conversationId: String, onDismiss: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var preview by remember { mutableStateOf<ContributionPreview?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    var reasoning by remember { mutableStateOf(false) }
    var fetched by remember { mutableStateOf(false) }
    var outcome by remember { mutableStateOf<String?>(null) }
    var notes by remember { mutableStateOf("") }
    var sending by remember { mutableStateOf(false) }
    var sent by remember { mutableStateOf<String?>(null) }
    val ratings = remember { mutableStateMapOf<Int, Int?>() }

    LaunchedEffect(reasoning, fetched) {
        try {
            preview = app.contributionPreview(conversationId, reasoning, fetched)
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = context.describe(e)
        }
    }

    fun rate(seq: Int, turnId: String?, value: Int?) {
        if (turnId == null) return
        scope.launch {
            try {
                app.rateTurn(turnId, value)
                ratings[seq] = value
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = context.describe(e)
            }
        }
    }

    fun send() {
        val p = preview ?: return
        sending = true
        scope.launch {
            try {
                val receipt = app.sendContribution(conversationId, p.bundleWith(outcome, notes, ratings))
                sent = receipt.id
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                error = context.describe(e)
            } finally {
                sending = false
            }
        }
    }

    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize()) {
            Scaffold(
                topBar = {
                    TopAppBar(
                        title = { Text(stringResource(R.string.contribute_this_conversation)) },
                        navigationIcon = { IconButton(onClick = onDismiss) { Icon(Icons.Filled.Close, stringResource(R.string.dismiss)) } },
                        actions = {
                            val p = preview
                            if (p != null && p.canSend) {
                                TextButton(onClick = ::send, enabled = !sending && p.accepted && p.blocked.isEmpty() && sent == null) {
                                    Text(stringResource(if (sending) R.string.contribute_sending else R.string.contribute_send))
                                }
                            }
                        },
                    )
                },
            ) { padding ->
                Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
                    val p = preview
                    LazyColumn(Modifier.widthIn(max = 720.dp).fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        error?.let { item { ErrorLine(it) } }
                        sent?.let { id -> item { Text(stringResource(R.string.contribute_sent, id), color = MaterialTheme.colorScheme.primary) } }
                        if (p == null) {
                            if (error == null) item { Loading() }
                            return@LazyColumn
                        }
                        item { Summary(p) }
                        items(p.blocked) { reason -> Text(blockedText(reason, p), color = CedarColors.red, style = MaterialTheme.typography.bodyMedium) }
                        if (!p.canSend) item { Text(stringResource(R.string.contribute_export_only), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                        else if (!p.accepted) item {
                            Column {
                                Text(stringResource(R.string.contribute_turn_on), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                                TextButton(onClick = { onDismiss(); app.showingSettings = true; app.showingContribute = true }) { Text(stringResource(R.string.contribute_open_settings)) }
                            }
                        }
                        item { Text(stringResource(R.string.contribute_intro), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                        item {
                            Column {
                                CheckRow(stringResource(R.string.contribute_include_reasoning), reasoning) { reasoning = it }
                                CheckRow(stringResource(R.string.contribute_include_fetched), fetched) { fetched = it }
                            }
                        }
                        item {
                            Column(Modifier.fillMaxWidth().border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(10.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text(stringResource(R.string.contribute_outcome_title), style = MaterialTheme.typography.labelLarge)
                                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                    for ((value, label) in listOf("succeeded" to R.string.contribute_outcome_yes, "partly" to R.string.contribute_outcome_partly, "failed" to R.string.contribute_outcome_no)) {
                                        FilterChip(selected = outcome == value, onClick = { outcome = if (outcome == value) null else value }, label = { Text(stringResource(label)) })
                                    }
                                }
                                OutlinedTextField(
                                    value = notes,
                                    onValueChange = { notes = it },
                                    modifier = Modifier.fillMaxWidth(),
                                    placeholder = { Text(stringResource(R.string.contribute_outcome_notes)) },
                                    minLines = 2,
                                )
                            }
                        }
                        val turns = p.turns()
                        items(turns, key = { it.seq }) { t ->
                            val rating = if (t.seq in ratings) ratings[t.seq] else t.rating
                            TurnCard(t, rating, standIns = p.standIns.values.flatten()) { value -> rate(t.seq, p.turnIds.getOrNull(t.seq - 1), value) }
                        }
                        if (p.standInCount > 0) item {
                            Text(stringResource(R.string.contribute_stand_ins_legend), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                            for ((kind, values) in p.standIns) {
                                Text("$kind: ${values.joinToString(", ")}", style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace, color = CedarColors.secondaryText)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Summary(p: ContributionPreview) {
    val d = p.dropped
    val parts = buildList {
        add(plural(R.plurals.contribute_turns, p.turns().size, p.turns().size))
        add(p.modelName)
        add(plural(R.plurals.contribute_stand_ins, p.standInCount, p.standInCount))
        if (d["memory"].bool == true) add(stringResource(R.string.contribute_memory_removed))
        d["attachments"].int?.takeIf { it > 0 }?.let { add(plural(R.plurals.contribute_attachments_left_out, it, it)) }
        d["fetched_bodies"].int?.takeIf { it > 0 }?.let { add(plural(R.plurals.contribute_fetched_left_out, it, it)) }
        d["reasoning"].int?.takeIf { it > 0 }?.let { add(plural(R.plurals.contribute_reasoning_left_out, it, it)) }
        add(stringResource(if (p.blocked.isEmpty()) R.string.contribute_ready else R.string.contribute_cannot_send))
    }
    Text(parts.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
}

@Composable
private fun blockedText(reason: String, p: ContributionPreview): String = when (reason) {
    "scheduled" -> stringResource(R.string.contribute_blocked_scheduled)
    "active" -> stringResource(R.string.contribute_blocked_active)
    "no_turns" -> stringResource(R.string.contribute_blocked_no_turns)
    "private_tools" -> stringResource(R.string.contribute_blocked_private_tools, p.privateTools.joinToString(", "))
    "run_gone" -> stringResource(R.string.contribute_blocked_run_gone)
    "secrets" -> stringResource(R.string.contribute_blocked_secrets) + "\n" + p.secrets.joinToString("\n")
    "not_consistent" -> stringResource(R.string.contribute_blocked_not_consistent) + "\n" + p.problems.joinToString("\n")
    "model_not_allowed" -> stringResource(R.string.contribute_blocked_model, p.modelName)
    else -> reason
}

@Composable
private fun CheckRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Checkbox(checked = checked, onCheckedChange = onChange)
        Text(label, style = MaterialTheme.typography.bodyMedium)
    }
}

/** One turn: the prompt, the agent's calls and text, with a thumb up or down. */
@Composable
private fun TurnCard(t: ContributionPreview.PreviewTurn, rating: Int?, standIns: List<String>, onRate: (Int?) -> Unit) {
    Column(Modifier.fillMaxWidth().border(1.dp, MaterialTheme.colorScheme.outlineVariant, RoundedCornerShape(10.dp)).padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(R.string.contribute_turn, t.seq), style = MaterialTheme.typography.labelLarge)
            Spacer(Modifier.padding(horizontal = 4.dp))
            Text("${t.status} · ${t.day}", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, modifier = Modifier.weight(1f))
            IconButton(onClick = { onRate(if (rating == 1) null else 1) }) {
                Icon(Icons.Outlined.ThumbUp, stringResource(R.string.contribute_rate_good), tint = if (rating == 1) MaterialTheme.colorScheme.primary else CedarColors.secondaryText)
            }
            IconButton(onClick = { onRate(if (rating == -1) null else -1) }) {
                Icon(Icons.Outlined.ThumbDown, stringResource(R.string.contribute_rate_bad), tint = if (rating == -1) CedarColors.red else CedarColors.secondaryText)
            }
        }
        Text(stringResource(R.string.contribute_you), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        Text(t.prompt, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
        Text(stringResource(R.string.contribute_agent), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        for ((name, input) in t.toolCalls) {
            Text("$name ${input.take(300)}", fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
        for (text in t.agentText) {
            Text(text, fontFamily = FontFamily.Monospace, style = MaterialTheme.typography.bodySmall)
        }
        val marked = standIns.filter { t.prompt.contains(it) || t.agentText.any { a -> a.contains(it) } || t.toolCalls.any { c -> c.second.contains(it) } }
        if (marked.isNotEmpty()) {
            Spacer(Modifier.height(2.dp))
            Text(stringResource(R.string.contribute_stand_ins_here, marked.joinToString(", ")), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.primary)
        }
    }
}
