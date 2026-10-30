package dev.cedar.cedar.ui

import androidx.compose.animation.animateColorAsState
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.semantics.Role
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
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.CenterAlignedTopAppBar
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
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.cedar.SidebarMode
import dev.cedar.core.Conversation
import kotlinx.coroutines.launch

/** The list: Chats | Starred | Scheduled, as iOS/Cedar/Features/Conversations/ConversationListView.swift. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConversationListScreen(app: AppModel) {
    val context = LocalContext.current
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
                actionError = context.describe(e)
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
            CenterAlignedTopAppBar(
                // The wordmark stands in for the title.
                title = { CedarWordmark() },
                navigationIcon = {
                    IconButton(onClick = { app.showingSettings = true }) { Icon(Icons.Outlined.Settings, stringResource(R.string.settings)) }
                },
                actions = {
                    IconButton(
                        onClick = {
                            app.sidebarMode = SidebarMode.CHATS
                            app.newChat()
                        },
                        enabled = app.settings.isConfigured,
                    ) { Icon(Icons.Outlined.EditNote, stringResource(R.string.new_chat)) }
                },
            )
        },
    ) { padding ->
        Column(Modifier.padding(padding).fillMaxSize()) {
            SidebarSwitch(
                mode = app.sidebarMode,
                scheduledUnread = app.scheduledUnread,
                onSelect = { app.sidebarMode = it },
                modifier = Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 4.dp, bottom = 10.dp),
            )
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
        RenameDialog(conversation.titleOr(stringResource(R.string.new_chat)), onDismiss = { renaming = null }) { title ->
            renaming = null
            if (title.isNotBlank()) act { app.rename(conversation, title.trim()) }
        }
    }
    ErrorDialog(stringResource(R.string.could_not_do_that), actionError) { actionError = null }
}

@Composable
private fun ChatRows(app: AppModel, onAction: (suspend () -> Unit) -> Unit, onRename: (Conversation) -> Unit) {
    val context = LocalContext.current
    if (app.conversations.isEmpty() && app.listError == null && !app.isRefreshing) {
        // In a scrollable column, so pulling to refresh still works on an empty list.
        LazyColumn(Modifier.fillMaxSize()) {
            item {
                EmptyState(
                    Icons.AutoMirrored.Outlined.Chat, stringResource(R.string.empty_chats_title), stringResource(R.string.empty_chats_text),
                    Modifier.padding(top = 80.dp),
                )
            }
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
                    MenuAction(
                        stringResource(if (starred) R.string.unstar else R.string.star),
                        if (starred) Icons.Outlined.StarBorder else Icons.Outlined.Star,
                    ) {
                        onAction { app.setStarred(conversation.id, !starred) }
                    },
                    MenuAction(stringResource(R.string.rename), Icons.Outlined.Edit) { onRename(conversation) },
                    MenuAction(stringResource(R.string.open_on_web), Icons.Outlined.OpenInBrowser) {
                        app.client?.let { openUrl(context, it.webURL(conversation.id)) }
                    },
                    MenuAction(stringResource(R.string.delete), Icons.Outlined.Delete, destructive = true) {
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
                        Icons.Outlined.StarBorder, stringResource(R.string.empty_starred_title), stringResource(R.string.empty_starred_text),
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
                    MenuAction(stringResource(R.string.unstar), Icons.Outlined.StarBorder) { onAction { app.setStarred(conversation.id, false) } },
                    MenuAction(stringResource(R.string.open_on_web), Icons.Outlined.OpenInBrowser) {
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
                .padding(horizontal = 8.dp)
                .clip(RoundedCornerShape(14.dp))
                .background(if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.12f) else androidx.compose.ui.graphics.Color.Transparent)
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
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            RowIcon(
                if (conversation.scheduleId != null) Icons.Outlined.Schedule else Icons.AutoMirrored.Outlined.Chat,
                tint = if (conversation.needsAnswer) CedarColors.clay else MaterialTheme.colorScheme.primary,
                running = running,
            )
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                val small = MaterialTheme.typography.bodySmall
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Text(
                            conversation.titleOr(stringResource(R.string.new_chat)),
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            fontWeight = if (conversation.unread) FontWeight.SemiBold else FontWeight.Normal,
                            modifier = Modifier.weight(1f, fill = false),
                        )
                        if (starred) StarMark()
                    }
                    Text(
                        relative(conversation.updatedAt),
                        style = small,
                        fontWeight = if (conversation.unread) FontWeight.SemiBold else FontWeight.Normal,
                        color = if (conversation.unread) MaterialTheme.colorScheme.primary else CedarColors.secondaryText,
                        maxLines = 1,
                    )
                }
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Row(
                        Modifier.weight(1f),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.spacedBy(4.dp),
                    ) {
                        if (conversation.scheduleId != null) {
                            Icon(Icons.Outlined.Schedule, null, Modifier.size(12.dp), tint = CedarColors.secondaryText)
                            Text(
                                conversation.scheduleTitle ?: stringResource(R.string.tab_scheduled),
                                style = small, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f, fill = false),
                            )
                        }
                        conversation.model?.takeIf { it.isNotEmpty() }?.let {
                            Text(
                                if (conversation.scheduleId != null) "· $it" else it,
                                style = small, color = CedarColors.secondaryText, maxLines = 1, overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f, fill = false),
                            )
                        }
                    }
                    if (conversation.needsAnswer) {
                        NeedsAnswerChip()
                    } else if (conversation.unread) {
                        UnreadDot(true, size = 9)
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
        stringResource(R.string.needs_answer),
        style = MaterialTheme.typography.labelSmall,
        fontWeight = FontWeight.SemiBold,
        color = CedarColors.clay,
        modifier = Modifier
            .clip(RoundedCornerShape(50))
            .background(CedarColors.clay.copy(alpha = 0.15f))
            .padding(horizontal = 8.dp, vertical = 3.dp),
    )
}

@Composable
private fun RenameDialog(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var title by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.rename_conversation)) },
        text = { OutlinedTextField(title, { title = it }, singleLine = true, label = { Text(stringResource(R.string.title)) }) },
        confirmButton = { TextButton(onClick = { onSave(title) }) { Text(stringResource(R.string.save)) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** The list's Chats | Starred | Scheduled switch: a pill with the picked one raised. */
@Composable
private fun SidebarSwitch(mode: SidebarMode, scheduledUnread: Int, onSelect: (SidebarMode) -> Unit, modifier: Modifier = Modifier) {
    Row(
        modifier
            .clip(RoundedCornerShape(50))
            .background(MaterialTheme.colorScheme.surfaceContainerHighest)
            .padding(3.dp)
            .selectableGroup(),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        listOf(
            SidebarMode.CHATS to R.string.tab_chats,
            SidebarMode.STARRED to R.string.tab_starred,
            SidebarMode.SCHEDULED to R.string.tab_scheduled,
        ).forEach { (value, label) ->
            val selected = mode == value
            val fill by animateColorAsState(
                if (selected) MaterialTheme.colorScheme.surfaceContainer else androidx.compose.ui.graphics.Color.Transparent,
                label = "segment",
            )
            Row(
                Modifier
                    .weight(1f)
                    .then(if (selected) Modifier.shadow(2.dp, RoundedCornerShape(50)) else Modifier)
                    .clip(RoundedCornerShape(50))
                    .background(fill)
                    .selectable(selected = selected, role = Role.Tab, onClick = { onSelect(value) })
                    .padding(vertical = 8.dp),
                horizontalArrangement = Arrangement.Center,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    stringResource(label),
                    style = MaterialTheme.typography.labelLarge,
                    fontWeight = if (selected) FontWeight.SemiBold else FontWeight.Medium,
                    color = if (selected) MaterialTheme.colorScheme.onSurface else CedarColors.secondaryText,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                if (value == SidebarMode.SCHEDULED && scheduledUnread > 0) {
                    Spacer(Modifier.width(5.dp))
                    Text(
                        "$scheduledUnread",
                        style = MaterialTheme.typography.labelSmall,
                        fontWeight = FontWeight.Bold,
                        color = MaterialTheme.colorScheme.onSecondary,
                        modifier = Modifier
                            .clip(RoundedCornerShape(50))
                            .background(CedarColors.clay)
                            .padding(horizontal = 6.dp, vertical = 1.dp),
                    )
                }
            }
        }
    }
}
