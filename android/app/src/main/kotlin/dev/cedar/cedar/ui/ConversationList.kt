package dev.cedar.cedar.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Chat
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.OpenInBrowser
import androidx.compose.material.icons.outlined.Schedule
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material.icons.outlined.Star
import androidx.compose.material.icons.outlined.EditNote
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.SidebarMode
import dev.cedar.core.Conversation
import kotlinx.coroutines.launch

/** The list: Chats | Starred | Scheduled, as iOS/Cedar/Features/Conversations/ConversationListView.swift. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConversationListScreen(app: AppModel) {
    val scope = rememberCoroutineScope()
    var actionError by remember { mutableStateOf<String?>(null) }
    var renaming by remember { mutableStateOf<Conversation?>(null) }

    fun act(block: suspend () -> Unit) {
        scope.launch {
            try {
                block()
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                actionError = e.message
            }
        }
    }

    LaunchedEffect(app.sidebarMode) {
        when (app.sidebarMode) {
            SidebarMode.CHATS -> {}
            // Stars set on the web or the desktop app since the last refresh.
            SidebarMode.STARRED -> app.refresh()
            SidebarMode.SCHEDULED -> app.refreshSchedules()
        }
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("cedar") },
                navigationIcon = {
                    IconButton(onClick = { app.showingSettings = true }) { Icon(Icons.Outlined.Settings, "Settings") }
                },
                actions = {
                    IconButton(
                        onClick = {
                            app.sidebarMode = SidebarMode.CHATS
                            app.newChat()
                        },
                        enabled = app.settings.isConfigured,
                    ) { Icon(Icons.Outlined.EditNote, "New Chat") }
                },
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)) {
                val modes = listOf(
                    SidebarMode.CHATS to "Chats",
                    SidebarMode.STARRED to "Starred",
                    SidebarMode.SCHEDULED to if (app.scheduledUnread > 0) "Scheduled (${app.scheduledUnread})" else "Scheduled",
                )
                modes.forEachIndexed { index, (mode, label) ->
                    SegmentedButton(
                        selected = app.sidebarMode == mode,
                        onClick = { app.sidebarMode = mode },
                        shape = SegmentedButtonDefaults.itemShape(index, modes.size),
                        icon = {},
                    ) { Text(label, maxLines = 1) }
                }
            }
            PullToRefreshBox(
                isRefreshing = app.isRefreshing,
                onRefresh = { scope.launch { app.refresh() } },
                modifier = Modifier.fillMaxSize(),
            ) {
                when (app.sidebarMode) {
                    SidebarMode.CHATS -> ChatRows(app, onAction = ::act, onRename = { renaming = it })
                    SidebarMode.STARRED -> StarredRows(app, onAction = ::act)
                    SidebarMode.SCHEDULED -> ScheduleRows(app, onAction = ::act)
                }
            }
        }
    }

    renaming?.let { conversation ->
        RenameDialog(conversation.displayTitle, onDismiss = { renaming = null }) { title ->
            renaming = null
            if (title.isNotBlank()) act { app.rename(conversation, title.trim()) }
        }
    }
    ErrorDialog("Could Not Do That", actionError) { actionError = null }
}

@Composable
private fun ChatRows(app: AppModel, onAction: (suspend () -> Unit) -> Unit, onRename: (Conversation) -> Unit) {
    val context = LocalContext.current
    if (app.conversations.isEmpty() && app.listError == null && !app.isRefreshing) {
        // In a scrollable column, so pulling to refresh still works on an empty list.
        LazyColumn(Modifier.fillMaxSize()) {
            item { EmptyState(Icons.AutoMirrored.Outlined.Chat, "No Conversations", "Start a new chat to run the agent.", Modifier.padding(top = 80.dp)) }
        }
        return
    }
    LazyColumn(Modifier.fillMaxSize()) {
        app.listError?.let { error -> item { ErrorLine(error) } }
        items(app.conversations, key = { it.id }) { conversation ->
            val starred = app.isStarred(conversation.id)
            ConversationRow(
                conversation = conversation,
                selected = app.selection == conversation.id,
                running = app.isRunning(conversation.id),
                starred = starred,
                onClick = { app.selection = conversation.id },
                menu = listOf(
                    MenuAction(if (starred) "Unstar" else "Star", if (starred) Icons.Outlined.StarBorder else Icons.Outlined.Star) {
                        onAction { app.setStarred(conversation.id, !starred) }
                    },
                    MenuAction("Rename", Icons.Outlined.Edit) { onRename(conversation) },
                    MenuAction("Open on the Web", Icons.Outlined.OpenInBrowser) {
                        app.client?.let { openUrl(context, it.webURL(conversation.id)) }
                    },
                    MenuAction("Delete", Icons.Outlined.Delete, destructive = true) {
                        onAction {
                            try {
                                app.delete(conversation)
                            } catch (e: Exception) {
                                app.refresh()
                                throw e
                            }
                        }
                    },
                ),
            )
        }
    }
}

/** Chats and scheduled runs together, most recently starred first. */
@Composable
private fun StarredRows(app: AppModel, onAction: (suspend () -> Unit) -> Unit) {
    val context = LocalContext.current
    if (app.starred.isEmpty() && app.starredError == null) {
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                if (app.starredLoaded) {
                    EmptyState(
                        Icons.Outlined.StarBorder, "No Starred Conversations",
                        "Star a chat or a scheduled run, here or on the web, to keep it at hand on every device.",
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
        app.starredError?.let { error -> item { ErrorLine(error) } }
        items(app.starred, key = { it.id }) { conversation ->
            ConversationRow(
                conversation = conversation,
                selected = app.selection == conversation.id,
                running = app.isRunning(conversation.id),
                starred = false,
                onClick = { app.selection = conversation.id },
                menu = listOf(
                    MenuAction("Unstar", Icons.Outlined.StarBorder) { onAction { app.setStarred(conversation.id, false) } },
                    MenuAction("Open on the Web", Icons.Outlined.OpenInBrowser) {
                        app.client?.let { openUrl(context, it.webURL(conversation.id)) }
                    },
                ),
            )
        }
    }
}

/** One entry of a row's long-press menu. */
data class MenuAction(
    val label: String,
    val icon: androidx.compose.ui.graphics.vector.ImageVector,
    val destructive: Boolean = false,
    val enabled: Boolean = true,
    val run: () -> Unit,
)

/** A row that opens on a tap and offers its actions on a long press. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun MenuRow(
    selected: Boolean,
    onClick: (() -> Unit)?,
    menu: List<MenuAction>,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box(modifier) {
        Box(
            Modifier
                .fillMaxWidth()
                .background(if (selected) MaterialTheme.colorScheme.secondaryContainer.copy(alpha = 0.5f) else androidx.compose.ui.graphics.Color.Transparent)
                .combinedClickable(
                    onClick = { onClick?.invoke() ?: run { if (menu.isNotEmpty()) open = true } },
                    onLongClick = if (menu.isNotEmpty()) ({ open = true }) else null,
                ),
        ) { content() }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            menu.forEach { action ->
                DropdownMenuItem(
                    text = { Text(action.label, color = if (action.destructive) CedarColors.red else androidx.compose.ui.graphics.Color.Unspecified) },
                    leadingIcon = { Icon(action.icon, null, tint = if (action.destructive) CedarColors.red else MaterialTheme.colorScheme.onSurfaceVariant) },
                    enabled = action.enabled,
                    onClick = {
                        open = false
                        action.run()
                    },
                )
            }
        }
    }
}

@Composable
private fun ConversationRow(
    conversation: Conversation,
    selected: Boolean,
    running: Boolean,
    /** Marks the row with a star (the Chats list; the Starred list needs none). */
    starred: Boolean,
    onClick: () -> Unit,
    menu: List<MenuAction>,
) {
    MenuRow(selected, onClick, menu) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.Top) {
            UnreadDot(conversation.unread, Modifier.padding(top = 7.dp))
            Spacer(Modifier.width(8.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(
                        conversation.displayTitle,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        fontWeight = if (conversation.unread) FontWeight.SemiBold else FontWeight.Normal,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    if (starred) StarMark()
                    if (running) CircularProgressIndicator(Modifier.size(12.dp), strokeWidth = 1.5.dp)
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    val small = MaterialTheme.typography.bodySmall
                    if (conversation.scheduleId != null) {
                        Icon(Icons.Outlined.Schedule, null, Modifier.size(12.dp), tint = CedarColors.secondaryText)
                        Text("${conversation.scheduleTitle ?: "Scheduled"} ·", style = small, color = CedarColors.secondaryText, maxLines = 1)
                    }
                    Text(relative(conversation.updatedAt), style = small, color = CedarColors.secondaryText, maxLines = 1)
                    if (conversation.needsAnswer) {
                        NeedsAnswerChip()
                    } else if (conversation.unread) {
                        Text("Unread", style = small, color = CedarColors.secondaryText)
                    }
                    conversation.model?.takeIf { it.isNotEmpty() }?.let {
                        Text("· $it", style = small, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }
    }
}

@Composable
fun UnreadDot(visible: Boolean, modifier: Modifier = Modifier, size: Int = 8) {
    Box(
        modifier
            .size(size.dp)
            .clip(CircleShape)
            .background(if (visible) MaterialTheme.colorScheme.primary else androidx.compose.ui.graphics.Color.Transparent),
    )
}

@Composable
fun NeedsAnswerChip() {
    Text(
        "Needs answer",
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.SemiBold,
        color = CedarColors.orange,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(CedarColors.orange.copy(alpha = 0.18f))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}

@Composable
private fun RenameDialog(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var title by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Rename Conversation") },
        text = { OutlinedTextField(title, { title = it }, singleLine = true, label = { Text("Title") }) },
        confirmButton = { TextButton(onClick = { onSave(title) }) { Text("Save") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
