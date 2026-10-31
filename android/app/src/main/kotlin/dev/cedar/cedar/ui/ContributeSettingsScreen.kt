package dev.cedar.cedar.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Switch
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.Contribution
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * Settings → Contribute: what a contribution contains, the one-time choices (license, pseudonym,
 * what to include by default), the switch that turns contributing on, and the receipts with
 * what the corpus says about each and Withdraw. Choices save at once, like Memory's deletions.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ContributeSettingsScreen(app: AppModel) {
    val scope = rememberCoroutineScope()
    var actionError by remember { mutableStateOf<String?>(null) }
    var withdrawing by remember { mutableStateOf<Contribution?>(null) }
    var refreshing by remember { mutableStateOf(false) }

    fun act(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                actionError = app.describe(e)
            }
        }
    }

    LaunchedEffect(Unit) { app.refreshContributions() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.contribute)) },
                navigationIcon = {
                    IconButton(onClick = { app.showingContribute = false }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            val s = app.contributionSettings
            LazyColumn(Modifier.widthIn(max = 640.dp).fillMaxSize(), contentPadding = androidx.compose.foundation.layout.PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                app.contributionsError?.let { error -> item { ErrorLine(error) } }
                if (s == null) {
                    if (app.contributionsError == null) item { Loading() }
                    return@LazyColumn
                }
                item { Text(stringResource(R.string.contribute_settings_description), style = MaterialTheme.typography.bodyMedium) }
                if (!s.available) item { Text(stringResource(R.string.contribute_unavailable), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                s.host?.let { host -> item { Text(stringResource(R.string.contribute_corpus, host), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) } }
                item { Text(stringResource(R.string.contribute_what), style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary) }
                for (id in listOf(R.string.contribute_what1, R.string.contribute_what2, R.string.contribute_what3, R.string.contribute_what4)) {
                    item { Text("• " + stringResource(id), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                }
                if (s.available) {
                    item { HorizontalDivider() }
                    item {
                        Text(stringResource(R.string.contribute_license), style = MaterialTheme.typography.labelLarge)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            for (license in s.licenses) {
                                FilterChip(
                                    selected = s.license == license,
                                    onClick = { act { app.updateContributionSettings(buildJsonObject { put("license", license) }) } },
                                    label = { Text(stringResource(if (license == "cc0") R.string.contribute_license_cc0 else R.string.contribute_license_ccby)) },
                                )
                            }
                        }
                    }
                    item {
                        Text(stringResource(R.string.contribute_pseudonym), style = MaterialTheme.typography.labelLarge)
                        Text(stringResource(R.string.contribute_pseudonym_desc), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
                        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Text(s.pseudonym ?: stringResource(R.string.contribute_unlinked), fontFamily = FontFamily.Monospace, modifier = Modifier.weight(1f))
                            TextButton(onClick = { act { app.updateContributionSettings(buildJsonObject { put("pseudonym", "new") }) } }) { Text(stringResource(R.string.contribute_new_pseudonym)) }
                            if (s.pseudonym != null) {
                                TextButton(onClick = { act { app.updateContributionSettings(buildJsonObject { put("pseudonym", "") }) } }) { Text(stringResource(R.string.contribute_drop_pseudonym)) }
                            }
                        }
                    }
                    item { SwitchRow(stringResource(R.string.contribute_default_reasoning), stringResource(R.string.contribute_default_reasoning_desc), s.includeReasoning) { on -> act { app.updateContributionSettings(buildJsonObject { put("include_reasoning", on) }) } } }
                    item { SwitchRow(stringResource(R.string.contribute_default_fetched), stringResource(R.string.contribute_default_fetched_desc), s.includeFetched) { on -> act { app.updateContributionSettings(buildJsonObject { put("include_fetched", on) }) } } }
                    item { SwitchRow(stringResource(R.string.contribute_enabled), stringResource(R.string.contribute_enabled_desc), s.enabled) { on -> act { app.updateContributionSettings(buildJsonObject { put("enabled", on) }) } } }
                }
                item { HorizontalDivider() }
                item {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.contribute_receipts), style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary, modifier = Modifier.weight(1f))
                        if (s.available && app.contributions.any { it.withdrawnAt == null }) {
                            TextButton(enabled = !refreshing, onClick = {
                                refreshing = true
                                act {
                                    try {
                                        app.refreshContributions(askCorpus = true)
                                    } finally {
                                        refreshing = false
                                    }
                                }
                            }) { Text(stringResource(R.string.contribute_refresh_status)) }
                        }
                    }
                }
                if (app.contributions.isEmpty()) item { Text(stringResource(R.string.contribute_no_receipts), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText) }
                items(app.contributions, key = { it.id }) { c ->
                    ReceiptRow(c) { withdrawing = c }
                    HorizontalDivider()
                }
            }
        }
    }

    withdrawing?.let { c ->
        AlertDialog(
            onDismissRequest = { withdrawing = null },
            title = { Text(stringResource(R.string.contribute_withdraw)) },
            text = { Text(stringResource(R.string.contribute_withdraw_confirm)) },
            confirmButton = {
                TextButton(onClick = {
                    withdrawing = null
                    act { app.withdrawContribution(c.id) }
                }) { Text(stringResource(R.string.contribute_withdraw), color = CedarColors.red) }
            },
            dismissButton = { TextButton(onClick = { withdrawing = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), actionError) { actionError = null }
}

@Composable
private fun SwitchRow(title: String, description: String, on: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(description, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
        }
        Switch(checked = on, onCheckedChange = onChange)
    }
}

@Composable
private fun ReceiptRow(c: Contribution, onWithdraw: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(c.title?.trim().orEmpty().ifEmpty { stringResource(R.string.contribute_untitled) })
            val released = if (c.releasedIn.isNotEmpty()) " · " + c.releasedIn.joinToString(", ") else ""
            Text("${relative(c.submittedAt)} · ${c.license} · ${c.id}$released", style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 2)
        }
        when {
            c.withdrawnAt != null -> Text(stringResource(R.string.contribute_withdrawn), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
            else -> {
                c.status?.let { Text(statusText(it), style = MaterialTheme.typography.bodySmall, color = if (it == "released") MaterialTheme.colorScheme.primary else CedarColors.secondaryText) }
                TextButton(onClick = onWithdraw) { Text(stringResource(R.string.contribute_withdraw)) }
            }
        }
    }
}

@Composable
private fun statusText(status: String): String = when (status) {
    "quarantined" -> stringResource(R.string.contribute_status_quarantined)
    "accepted" -> stringResource(R.string.contribute_status_accepted)
    "held" -> stringResource(R.string.contribute_status_held)
    "rejected" -> stringResource(R.string.contribute_status_rejected)
    "released" -> stringResource(R.string.contribute_status_released)
    else -> stringResource(R.string.contribute_status_unknown)
}
