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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.BookmarkBorder
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.Unarchive
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.AssistChip
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
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
import dev.cedar.core.MemoryFilter
import dev.cedar.core.MemoryItem
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch

/**
 * Settings → Memory: what Cedar remembers about the user across conversations, as the web chat's
 * Settings → Memory shows it. At the top, the profile Cedar reads before every reply (written by
 * Cedar, or by the user: edit it and it stays as written). Below, chips for the memories about
 * the user, past conversations, ways of working, or the archive, newest first, each with the
 * words it was taken from and the conversation it came from. A long press offers Pin, Archive or
 * Restore, and Delete; Delete all in the top bar forgets everything after asking. Changes apply
 * at once, like the language: the next message, in any conversation, sees them.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MemoryScreen(app: AppModel) {
    val scope = rememberCoroutineScope()
    var confirmAll by remember { mutableStateOf(false) }
    var actionError by remember { mutableStateOf<String?>(null) }
    var profileDraft by remember { mutableStateOf("") }
    var profileLoaded by remember { mutableStateOf<String?>(null) }
    var savingProfile by remember { mutableStateOf(false) }

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

    LaunchedEffect(app.memoryFilter) { app.refreshMemory() }
    // The draft follows the saved profile until the user types.
    val saved = app.memoryProfile?.text.orEmpty()
    LaunchedEffect(saved) {
        if (profileLoaded != saved) {
            profileDraft = saved
            profileLoaded = saved
        }
    }

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
                    if (app.memoryItems.isNotEmpty() || app.memoryProfile != null) {
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
                    else -> {
                        item {
                            Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                                Text(stringResource(R.string.memory_profile), style = MaterialTheme.typography.labelMedium, color = CedarColors.secondaryText)
                                OutlinedTextField(
                                    value = profileDraft,
                                    onValueChange = { profileDraft = it },
                                    modifier = Modifier.fillMaxWidth(),
                                    placeholder = { Text(stringResource(R.string.memory_profile_placeholder)) },
                                    minLines = 3,
                                )
                                val profile = app.memoryProfile
                                Text(
                                    when {
                                        profile == null -> stringResource(R.string.memory_profile_none)
                                        profile.userEdited -> stringResource(R.string.memory_profile_by_you, relative(profile.updatedAt))
                                        else -> stringResource(R.string.memory_profile_by_cedar, relative(profile.updatedAt))
                                    },
                                    style = MaterialTheme.typography.bodySmall,
                                    color = CedarColors.secondaryText,
                                )
                                if (profileDraft != saved) {
                                    TextButton(
                                        enabled = !savingProfile,
                                        onClick = {
                                            savingProfile = true
                                            scope.launch {
                                                try {
                                                    app.saveMemoryProfile(profileDraft)
                                                    profileLoaded = null
                                                } catch (e: CancellationException) {
                                                    throw e
                                                } catch (e: Exception) {
                                                    actionError = app.describe(e)
                                                } finally {
                                                    savingProfile = false
                                                }
                                            }
                                        },
                                    ) { Text(stringResource(R.string.memory_save_profile)) }
                                }
                            }
                            HorizontalDivider()
                        }
                        item {
                            Row(
                                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
                                horizontalArrangement = Arrangement.spacedBy(8.dp),
                            ) {
                                for ((filter, label) in listOf(
                                    MemoryFilter.ABOUT to R.string.memory_view_about,
                                    MemoryFilter.CONVERSATIONS to R.string.memory_view_conversations,
                                    MemoryFilter.PROCEDURES to R.string.memory_view_procedures,
                                    MemoryFilter.ARCHIVE to R.string.memory_view_archive,
                                )) {
                                    FilterChip(
                                        selected = app.memoryFilter == filter,
                                        onClick = { app.memoryFilter = filter },
                                        label = { Text(stringResource(label)) },
                                    )
                                }
                            }
                        }
                        if (app.memoryItems.isEmpty()) {
                            item {
                                Text(
                                    stringResource(
                                        when (app.memoryFilter) {
                                            MemoryFilter.ABOUT -> R.string.memory_empty_text
                                            MemoryFilter.CONVERSATIONS -> R.string.memory_empty_conversations
                                            MemoryFilter.PROCEDURES -> R.string.memory_empty_procedures
                                            MemoryFilter.ARCHIVE -> R.string.memory_empty_archive
                                        },
                                    ),
                                    style = MaterialTheme.typography.bodySmall,
                                    color = CedarColors.secondaryText,
                                    modifier = Modifier.padding(16.dp),
                                )
                            }
                        } else {
                            item {
                                val count = app.memoryItems.size
                                Text(
                                    plural(R.plurals.memory_count, count, count),
                                    style = MaterialTheme.typography.labelMedium,
                                    color = CedarColors.secondaryText,
                                    modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp),
                                )
                            }
                            // Newest first here; the agent reads them oldest first.
                            items(app.memoryItems.asReversed(), key = { it.id }) { item ->
                                MemoryRow(
                                    app, item,
                                    onPin = { act { app.updateMemory(item, pinned = !item.pinned) } },
                                    onArchive = { act { app.updateMemory(item, archived = item.isActive) } },
                                    onDelete = { act { app.deleteMemory(item) } },
                                )
                                HorizontalDivider()
                            }
                        }
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
    }

    if (confirmAll) {
        AlertDialog(
            onDismissRequest = { confirmAll = false },
            title = { Text(stringResource(R.string.memory_delete_all_title)) },
            text = { Text(stringResource(R.string.memory_delete_all_everything)) },
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

/**
 * One memory: its kind and text, the words it was taken from, when it was saved and where. A tap
 * opens its conversation; a long press offers Pin, Archive or Restore, and Delete.
 */
@Composable
private fun MemoryRow(app: AppModel, item: MemoryItem, onPin: () -> Unit, onArchive: () -> Unit, onDelete: () -> Unit) {
    val open = item.conversationId?.let { id ->
        {
            app.showingMemory = false
            app.showingSettings = false
            app.selection = id
        }
    }
    val menu = buildList {
        if (item.isActive && item.isAboutUser) {
            add(MenuAction(stringResource(if (item.pinned) R.string.memory_unpin else R.string.memory_pin), Icons.Outlined.PushPin, run = onPin))
        }
        if (item.isActive) {
            add(MenuAction(stringResource(R.string.memory_archive), Icons.Outlined.Archive, run = onArchive))
        } else if (item.supersededBy == null) {
            add(MenuAction(stringResource(R.string.memory_restore), Icons.Outlined.Unarchive, run = onArchive))
        }
        add(MenuAction(stringResource(R.string.delete), Icons.Outlined.Delete, destructive = true, run = onDelete))
    }
    MenuRow(selected = false, onClick = open, menu = menu) {
        Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                AssistChip(onClick = {}, enabled = false, label = { Text(stringResource(kindLabel(item.kind))) })
                if (item.pinned) Icon(Icons.Outlined.PushPin, stringResource(R.string.memory_pinned), tint = CedarColors.secondaryText)
            }
            Text(item.text)
            item.support?.takeIf { it != item.text }?.let { quote ->
                Text(stringResource(R.string.memory_said, quote), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 2)
            }
            val bits = buildList {
                add(relative(item.validFrom ?: item.createdAt))
                item.validUntil?.let { add(stringResource(R.string.memory_until, relative(it))) }
                if (item.supersededBy != null) add(stringResource(R.string.memory_superseded))
                else if (item.archivedAt != null) add(stringResource(R.string.memory_archived))
                if (item.recallCount > 0) add(plural(R.plurals.memory_used, item.recallCount, item.recallCount))
                add(
                    when {
                        item.conversationId != null -> item.title?.trim().orEmpty().ifEmpty { stringResource(R.string.new_chat) }
                        item.source == "consolidation" -> stringResource(R.string.memory_from_consolidation)
                        else -> stringResource(R.string.memory_no_source)
                    },
                )
            }
            Text(bits.joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText, maxLines = 1)
        }
    }
}

private fun kindLabel(kind: String): Int = when (kind) {
    "preference" -> R.string.memory_kind_preference
    "instruction" -> R.string.memory_kind_instruction
    "temporary" -> R.string.memory_kind_temporary
    "episode" -> R.string.memory_kind_episode
    "procedure" -> R.string.memory_kind_procedure
    else -> R.string.memory_kind_fact
}
