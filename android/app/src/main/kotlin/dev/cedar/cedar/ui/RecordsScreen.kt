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
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.TableChart
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.RecordRow
import dev.cedar.core.RecordSet
import dev.cedar.core.RecordSetDetail
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive

/**
 * Settings → Records: the data Cedar keeps for the user across conversations (a running log,
 * weekly measurements), in named sets, as the web chat's Settings → Records lists them. A tap
 * opens a set's rows; a long press deletes a set, and Delete all in the top bar every set after
 * asking. Deletions apply at once, like the language: the next message, in any conversation, goes
 * without them.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RecordsScreen(app: AppModel) {
    val scope = rememberCoroutineScope()
    var confirmAll by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }

    fun act(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                actionError = app.describe(e)
                app.refreshRecords()
            }
        }
    }

    LaunchedEffect(Unit) { app.refreshRecords() }

    val openSet = app.showingRecordSet
    if (openSet != null) {
        RecordSetScreen(app, openSet)
        return
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.records)) },
                navigationIcon = {
                    IconButton(onClick = { app.showingRecords = false }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                    }
                },
                actions = {
                    if (app.recordSets.isNotEmpty()) {
                        TextButton(onClick = { confirmAll = true }) { Text(stringResource(R.string.records_delete_all), color = CedarColors.red) }
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            LazyColumn(Modifier.widthIn(max = 640.dp).fillMaxSize()) {
                app.recordsError?.let { error -> item { ErrorLine(error) } }
                when {
                    !app.recordsLoaded -> if (app.recordsError == null) item { Loading() }
                    !app.recordsSupported -> item {
                        EmptyState(
                            Icons.Outlined.TableChart, stringResource(R.string.records), stringResource(R.string.records_unsupported),
                            Modifier.padding(top = 80.dp),
                        )
                    }
                    app.recordSets.isEmpty() -> item {
                        EmptyState(
                            Icons.Outlined.TableChart, stringResource(R.string.records_empty_title), stringResource(R.string.records_empty_text),
                            Modifier.padding(top = 80.dp),
                        )
                    }
                    else -> {
                        item {
                            val max = app.recordsLimits?.maxSets ?: 0
                            val count = app.recordSets.size
                            Text(
                                if (max > 0) stringResource(R.string.records_count_of, count, max) else plural(R.plurals.records_count, count, count),
                                style = MaterialTheme.typography.labelMedium,
                                color = CedarColors.secondaryText,
                                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                            )
                        }
                        // Most recently saved first.
                        items(app.recordSets.sortedByDescending { it.updatedAt }, key = { it.id }) { set ->
                            RecordSetRow(set, onOpen = { app.showingRecordSet = set }) { act { app.deleteRecordSet(set) } }
                            HorizontalDivider()
                        }
                    }
                }
                if (app.recordsLoaded && app.recordsSupported) {
                    item {
                        Text(
                            stringResource(R.string.records_footnote),
                            style = MaterialTheme.typography.bodySmall,
                            color = CedarColors.secondaryText,
                            modifier = Modifier.padding(16.dp),
                        )
                    }
                }
            }
        }
    }

    if (confirmAll) {
        val count = app.recordSets.size
        AlertDialog(
            onDismissRequest = { confirmAll = false },
            title = { Text(stringResource(R.string.records_delete_all_title)) },
            text = { Text(plural(R.plurals.records_delete_all_text, count, count)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmAll = false
                    act { app.deleteAllRecords() }
                }) { Text(stringResource(R.string.records_delete_all), color = CedarColors.red) }
            },
            dismissButton = { TextButton(onClick = { confirmAll = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), actionError) { actionError = null }
}

/** One set: its name, description, size, key, columns and when it was last saved. A tap opens its rows; a long press offers Delete. */
@Composable
private fun RecordSetRow(set: RecordSet, onOpen: () -> Unit, onDelete: () -> Unit) {
    MenuRow(
        selected = false,
        onClick = onOpen,
        menu = listOf(MenuAction(stringResource(R.string.delete), Icons.Outlined.Delete, destructive = true, run = onDelete)),
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(set.name, fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold)
                if (set.healthData) {
                    Text(
                        stringResource(R.string.records_health),
                        style = MaterialTheme.typography.labelSmall,
                        color = CedarColors.secondaryText,
                    )
                }
            }
            if (set.description.isNotEmpty()) Text(set.description, style = MaterialTheme.typography.bodyMedium)
            val rows = plural(R.plurals.records_rows, set.count, set.count)
            Text(
                "$rows · ${relative(set.updatedAt)}",
                style = MaterialTheme.typography.bodySmall,
                color = CedarColors.secondaryText,
                maxLines = 1,
            )
            if (set.columns.isNotEmpty()) {
                Text(
                    set.columns.joinToString(", "),
                    style = MaterialTheme.typography.bodySmall,
                    fontFamily = FontFamily.Monospace,
                    color = CedarColors.secondaryText,
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}

/**
 * One set's rows, newest first: each row's fields, when it was saved, and the conversation it
 * came from (a tap opens it).
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun RecordSetScreen(app: AppModel, set: RecordSet) {
    var detail by remember(set.id) { mutableStateOf<RecordSetDetail?>(null) }
    var error by remember(set.id) { mutableStateOf<String?>(null) }

    LaunchedEffect(set.id) {
        try {
            detail = app.loadRecordSet(set.name)
            error = null
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            error = app.describe(e)
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(set.name, fontFamily = FontFamily.Monospace) },
                navigationIcon = {
                    IconButton(onClick = { app.showingRecordSet = null }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            LazyColumn(Modifier.widthIn(max = 640.dp).fillMaxSize()) {
                error?.let { message -> item { ErrorLine(message) } }
                val loaded = detail
                when {
                    loaded == null -> if (error == null) item { Loading() }
                    loaded.records.isEmpty() -> item {
                        EmptyState(Icons.Outlined.TableChart, stringResource(R.string.records_no_rows_title), stringResource(R.string.records_no_rows_text), Modifier.padding(top = 80.dp))
                    }
                    else -> {
                        val columns = loaded.set.columns.ifEmpty { loaded.records.flatMap { it.data.keys }.distinct() }
                        item {
                            val rows = plural(R.plurals.records_rows, loaded.records.size, loaded.records.size)
                            val key = loaded.set.keyFields.takeIf { it.isNotEmpty() }?.let { " · " + stringResource(R.string.records_key, it.joinToString(", ")) }.orEmpty()
                            Text(
                                rows + key,
                                style = MaterialTheme.typography.labelMedium,
                                color = CedarColors.secondaryText,
                                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                            )
                        }
                        items(loaded.records.asReversed(), key = { it.id }) { record ->
                            RecordRowView(app, record, columns)
                            HorizontalDivider()
                        }
                        if (loaded.set.description.isNotEmpty()) {
                            item {
                                Text(
                                    loaded.set.description,
                                    style = MaterialTheme.typography.bodySmall,
                                    color = CedarColors.secondaryText,
                                    modifier = Modifier.padding(16.dp),
                                )
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun RecordRowView(app: AppModel, record: RecordRow, columns: List<String>) {
    val open = record.conversationId?.let { id ->
        {
            app.showingRecordSet = null
            app.showingRecords = false
            app.showingSettings = false
            app.selection = id
        }
    }
    MenuRow(selected = false, onClick = open, menu = emptyList()) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            for (column in columns) {
                val value = record.data[column]
                if (value == null || value is JsonNull) continue
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.Top) {
                    Text(
                        column,
                        style = MaterialTheme.typography.bodySmall,
                        fontFamily = FontFamily.Monospace,
                        color = CedarColors.secondaryText,
                        modifier = Modifier.widthIn(min = 72.dp),
                    )
                    Text(displayText(value), style = MaterialTheme.typography.bodyMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
                }
            }
            val source = when {
                record.conversationId != null -> record.title?.trim().orEmpty().ifEmpty { stringResource(R.string.new_chat) }
                record.runId != null -> stringResource(R.string.records_from_elsewhere)
                else -> stringResource(R.string.records_no_source)
            }
            Text(
                "${relative(record.updatedAt)} · $source",
                style = MaterialTheme.typography.bodySmall,
                color = CedarColors.secondaryText,
                maxLines = 1,
            )
        }
    }
}

/** A value as a row shows it: strings as they are, whole numbers without `.0`, the rest as JSON. */
private fun displayText(value: JsonElement): String = when (value) {
    is JsonPrimitive -> if (value.isString) value.content else value.content.removeSuffix(".0")
    is JsonObject, is JsonArray -> value.toString()
    else -> ""
}
