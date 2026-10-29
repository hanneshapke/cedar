package dev.cedar.cedar.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.BookmarkBorder
import androidx.compose.material.icons.outlined.Delete
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
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.core.MemoryItem
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Settings → Memory: what Cedar remembers about the user across conversations, newest first, as
 * the web chat's Settings → Memory lists it. A long press deletes one, and Delete all in the top
 * bar forgets everything after asking. Deletions apply at once, like the language: the next
 * message, in any conversation, goes without them.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MemoryScreen(app: AppModel) {
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
                app.refreshMemory()
            }
        }
    }

    LaunchedEffect(Unit) { app.refreshMemory() }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.memory)) },
                navigationIcon = {
                    IconButton(onClick = { app.showingMemory = false }) {
                        Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back))
                    }
                },
                actions = {
                    if (app.memoryItems.isNotEmpty()) {
                        TextButton(onClick = { confirmAll = true }) { Text(stringResource(R.string.memory_delete_all), color = CedarColors.red) }
                    }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            LazyColumn(Modifier.widthIn(max = 640.dp).fillMaxSize()) {
                app.memoryError?.let { error -> item { ErrorLine(error) } }
                when {
                    !app.memoryLoaded -> if (app.memoryError == null) item { Loading() }
                    !app.memorySupported -> item {
                        EmptyState(
                            Icons.Outlined.BookmarkBorder, stringResource(R.string.memory), stringResource(R.string.memory_unsupported),
                            Modifier.padding(top = 80.dp),
                        )
                    }
                    app.memoryItems.isEmpty() -> item {
                        EmptyState(
                            Icons.Outlined.BookmarkBorder, stringResource(R.string.memory_empty_title), stringResource(R.string.memory_empty_text),
                            Modifier.padding(top = 80.dp),
                        )
                    }
                    else -> {
                        item {
                            val max = app.memoryLimits?.maxItems ?: 0
                            val count = app.memoryItems.size
                            Text(
                                if (max > 0) stringResource(R.string.memory_count_of, count, max) else plural(R.plurals.memory_count, count, count),
                                style = MaterialTheme.typography.labelMedium,
                                color = CedarColors.secondaryText,
                                modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                            )
                        }
                        // Newest first here; the agent reads them oldest first.
                        items(app.memoryItems.asReversed(), key = { it.id }) { item ->
                            MemoryRow(app, item) { act { app.deleteMemory(item) } }
                            HorizontalDivider()
                        }
                    }
                }
                if (app.memoryLoaded && app.memorySupported) {
                    item {
                        Text(
                            stringResource(R.string.memory_footnote),
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
        val count = app.memoryItems.size
        AlertDialog(
            onDismissRequest = { confirmAll = false },
            title = { Text(stringResource(R.string.memory_delete_all_title)) },
            text = { Text(plural(R.plurals.memory_delete_all_text, count, count)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmAll = false
                    act { app.deleteAllMemory() }
                }) { Text(stringResource(R.string.memory_delete_all), color = CedarColors.red) }
            },
            dismissButton = { TextButton(onClick = { confirmAll = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), actionError) { actionError = null }
}

/** One memory: its text, when it was saved and where. A tap opens its conversation; a long press offers Delete. */
@Composable
private fun MemoryRow(app: AppModel, item: MemoryItem, onDelete: () -> Unit) {
    val open = item.conversationId?.let { id ->
        {
            app.showingMemory = false
            app.showingSettings = false
            app.selection = id
        }
    }
    MenuRow(
        selected = false,
        onClick = open,
        menu = listOf(MenuAction(stringResource(R.string.delete), Icons.Outlined.Delete, destructive = true, run = onDelete)),
    ) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(item.text)
            val source = if (item.conversationId != null) {
                item.title?.trim().orEmpty().ifEmpty { stringResource(R.string.new_chat) }
            } else {
                stringResource(R.string.memory_no_source)
            }
            Text(
                "${relative(item.createdAt)} · $source",
                style = MaterialTheme.typography.bodySmall,
                color = CedarColors.secondaryText,
                maxLines = 1,
            )
        }
    }
}
