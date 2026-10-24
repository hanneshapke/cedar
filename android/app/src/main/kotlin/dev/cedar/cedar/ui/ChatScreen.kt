package dev.cedar.cedar.ui

import android.graphics.BitmapFactory
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material.icons.outlined.Description
import androidx.compose.material.icons.outlined.Eco
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.OpenInBrowser
import androidx.compose.material.icons.outlined.PhotoLibrary
import androidx.compose.material.icons.outlined.StarBorder
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.ChatModel
import dev.cedar.core.AttachmentInfo
import dev.cedar.core.OutgoingAttachment
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** A conversation: its turns as they stream in, and the composer (iOS/Cedar/Features/Chat/ChatView.swift). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ChatScreen(app: AppModel, chat: ChatModel, onBack: (() -> Unit)?) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var starError by remember { mutableStateOf<String?>(null) }
    val listState = rememberLazyListState()

    LaunchedEffect(chat) { chat.start() }
    LaunchedEffect(chat.conversationId) { chat.conversationId?.let { if (app.conversation(it) != null) app.markSeen(it) } }

    val conversationId = chat.conversationId
    val title = conversationId?.let { app.conversation(it) }?.displayTitle
        ?: chat.turns.firstOrNull()?.prompt?.lineSequence()?.firstOrNull { it.isNotBlank() }?.trim()?.let { if (it.length > 80) it.take(80) + "…" else it }
        ?: "New Chat"
    val webURL = conversationId?.let { id -> app.client?.webURL(id) }
    val openQuestions = chat.openQuestions

    // Keep the newest content in view as it arrives.
    val signature = listOf(chat.turns.size, chat.turns.lastOrNull()?.lastSeq ?: 0, if (chat.pendingPrompt == null) 0 else 1, if (openQuestions == null) 0 else 1)
    LaunchedEffect(signature) {
        val count = listState.layoutInfo.totalItemsCount
        if (count > 0) listState.animateScrollToItem(count - 1)
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                navigationIcon = {
                    if (onBack != null) IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
                },
                actions = {
                    if (conversationId != null) {
                        val starred = app.isStarred(conversationId)
                        IconButton(onClick = {
                            scope.launch {
                                try {
                                    app.setStarred(conversationId, !starred)
                                } catch (e: kotlinx.coroutines.CancellationException) {
                                    throw e
                                } catch (e: Exception) {
                                    starError = e.message
                                }
                            }
                        }) {
                            if (starred) Icon(Icons.Filled.Star, "Unstar", tint = CedarColors.yellow) else Icon(Icons.Outlined.StarBorder, "Star")
                        }
                    }
                    if (webURL != null) {
                        IconButton(onClick = { openUrl(context, webURL) }) { Icon(Icons.Outlined.OpenInBrowser, "Open on the web") }
                    }
                },
            )
        },
        bottomBar = { Composer(chat) },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            LazyColumn(
                state = listState,
                modifier = Modifier.widthIn(max = 820.dp).fillMaxSize(),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 16.dp, vertical = 12.dp),
                verticalArrangement = Arrangement.spacedBy(20.dp),
            ) {
                if (chat.turns.isEmpty() && chat.pendingPrompt == null) {
                    if (chat.isLoading) {
                        item { Loading(Modifier.padding(top = 40.dp)) }
                    } else if (conversationId == null) {
                        item { Greeting() }
                    }
                }
                items(chat.turns, key = { it.id }) { turn ->
                    TurnView(turn, chat, webURL, isLast = turn.id == chat.turns.lastOrNull()?.id)
                }
                if (openQuestions != null && chat.pendingPrompt == null) {
                    // A fresh form for each turn's questions.
                    item(key = "questions-${chat.turns.lastOrNull()?.id}") {
                        QuestionsView(openQuestions) { answers -> chat.answer(openQuestions, answers) }
                    }
                }
                chat.pendingPrompt?.let { pending ->
                    item(key = "pending") {
                        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                            UserBubble(pending, chat.pendingAttachments)
                            CircularProgressIndicator(Modifier.size(20.dp).padding(start = 4.dp), strokeWidth = 2.dp)
                        }
                    }
                }
                chat.errorMessage?.let { error ->
                    item(key = "error") { ErrorBanner(error) { chat.errorMessage = null } }
                }
            }
        }
    }
    ErrorDialog("Could Not Change the Star", starError) { starError = null }
}

@Composable
private fun Greeting() {
    Column(Modifier.fillMaxWidth().padding(top = 80.dp, start = 16.dp, end = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Icon(Icons.Outlined.Eco, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.primary)
        Spacer(Modifier.size(12.dp))
        Text("What should the agent do?", style = MaterialTheme.typography.titleLarge)
        Spacer(Modifier.size(8.dp))
        Text(
            "It can search the web, write files and run code in its own sandbox. Follow-ups keep its files.",
            style = MaterialTheme.typography.bodyMedium,
            color = CedarColors.secondaryText,
            textAlign = TextAlign.Center,
        )
    }
}

@Composable
fun UserBubble(text: String, attachments: List<AttachmentInfo> = emptyList()) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        Spacer(Modifier.size(48.dp))
        Column(
            Modifier
                .clip(RoundedCornerShape(18.dp))
                .background(MaterialTheme.colorScheme.surfaceContainerHighest)
                .padding(horizontal = 14.dp, vertical = 10.dp),
            horizontalAlignment = Alignment.End,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            SelectionContainer { Text(text) }
            if (attachments.isNotEmpty()) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    attachments.forEach { file ->
                        Row(
                            Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.surface).padding(horizontal = 8.dp, vertical = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Icon(if (file.mediaType == OutgoingAttachment.PDF) Icons.Outlined.Description else Icons.Outlined.Image, null, Modifier.size(14.dp))
                            Spacer(Modifier.size(4.dp))
                            Text(file.name, style = MaterialTheme.typography.labelSmall, maxLines = 1)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun ErrorBanner(message: String, dismiss: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(12.dp)).background(CedarColors.red.copy(alpha = 0.1f)).padding(12.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(Icons.Outlined.WarningAmber, null, Modifier.size(18.dp), tint = CedarColors.red)
        Spacer(Modifier.size(8.dp))
        Text(message, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = CedarColors.red)
        IconButton(onClick = dismiss, modifier = Modifier.size(24.dp)) { Icon(Icons.Filled.Close, "Dismiss", tint = CedarColors.red) }
    }
}

@Composable
private fun Composer(chat: ChatModel) {
    val context = LocalContext.current
    var menu by remember { mutableStateOf(false) }
    val photos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(OutgoingAttachment.MAX_FILES)) { uris ->
        chat.addFiles(context, uris)
    }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        chat.addFiles(context, uris)
    }
    Surface(color = MaterialTheme.colorScheme.surfaceContainer, tonalElevation = 2.dp) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().imePadding().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (chat.attachments.isNotEmpty()) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    chat.attachments.forEach { file -> AttachmentChip(file) { chat.removeAttachment(file) } }
                }
            }
            Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box {
                    IconButton(
                        onClick = { menu = true },
                        enabled = !chat.isActive && !chat.isAddingAttachments && chat.attachments.size < OutgoingAttachment.MAX_FILES,
                    ) {
                        if (chat.isAddingAttachments) {
                            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        } else {
                            Icon(Icons.Outlined.AttachFile, if (chat.acceptsImages) "Attach images or PDFs" else "Attach PDFs")
                        }
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        // Photos only for a model that takes images; PDFs for any.
                        if (chat.acceptsImages) {
                            DropdownMenuItem(
                                text = { Text("Photos") },
                                leadingIcon = { Icon(Icons.Outlined.PhotoLibrary, null) },
                                onClick = {
                                    menu = false
                                    photos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                                },
                            )
                        }
                        DropdownMenuItem(
                            text = { Text(if (chat.acceptsImages) "Choose Files" else "Choose PDFs") },
                            leadingIcon = { Icon(Icons.Outlined.Folder, null) },
                            onClick = {
                                menu = false
                                files.launch(if (chat.acceptsImages) arrayOf(OutgoingAttachment.PDF, "image/*") else arrayOf(OutgoingAttachment.PDF))
                            },
                        )
                    }
                }
                OutlinedTextField(
                    value = chat.draft,
                    onValueChange = { chat.draft = it },
                    modifier = Modifier.weight(1f),
                    placeholder = { Text(if (chat.openQuestions == null) "Message cedar" else "Or answer in your own words") },
                    maxLines = 8,
                    shape = RoundedCornerShape(20.dp),
                )
                if (chat.isActive) {
                    FilledIconButton(onClick = { chat.stop() }, enabled = chat.pendingPrompt == null) { Icon(Icons.Filled.Stop, "Stop") }
                } else {
                    FilledIconButton(onClick = { chat.send() }, enabled = chat.canSend) { Icon(Icons.Filled.ArrowUpward, "Send") }
                }
            }
        }
    }
}

/** A file picked for the next message, with a thumbnail for images and a remove button. */
@Composable
private fun AttachmentChip(file: OutgoingAttachment, remove: () -> Unit) {
    val thumbnail by produceState<ImageBitmap?>(null, file.id) {
        if (file.isImage) {
            value = withContext(Dispatchers.Default) {
                val options = BitmapFactory.Options().apply { inSampleSize = 8 }
                BitmapFactory.decodeByteArray(file.data, 0, file.data.size, options)?.asImageBitmap()
            }
        }
    }
    Row(
        Modifier.clip(RoundedCornerShape(50)).background(MaterialTheme.colorScheme.surfaceContainerHighest).padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val image = thumbnail
        if (image != null) {
            Image(image, null, Modifier.size(28.dp).clip(RoundedCornerShape(6.dp)), contentScale = ContentScale.Crop)
        } else {
            Icon(if (file.isImage) Icons.Outlined.Image else Icons.Outlined.Description, null, Modifier.size(28.dp).padding(4.dp), tint = CedarColors.secondaryText)
        }
        Spacer(Modifier.size(6.dp))
        Column(Modifier.widthIn(max = 140.dp)) {
            Text(file.name, style = MaterialTheme.typography.labelMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
            Text(OutgoingAttachment.format(file.data.size.toLong()), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        }
        IconButton(onClick = remove, modifier = Modifier.size(28.dp)) { Icon(Icons.Outlined.Cancel, "Remove ${file.name}", tint = CedarColors.secondaryText) }
    }
}
