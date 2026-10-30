package dev.cedar.cedar.ui

import android.graphics.BitmapFactory
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.text.font.FontWeight
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
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.outlined.Cancel
import androidx.compose.material.icons.outlined.Description
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
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Surface
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.ChatModel
import dev.cedar.cedar.R
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
    var showingFiles by remember { mutableStateOf(false) }
    val listState = rememberLazyListState()

    LaunchedEffect(chat) { chat.start() }
    LaunchedEffect(chat.conversationId) { chat.conversationId?.let { if (app.conversation(it) != null) app.markSeen(it) } }

    val conversationId = chat.conversationId
    val newChat = stringResource(R.string.new_chat)
    val title = conversationId?.let { app.conversation(it) }?.titleOr(newChat)
        ?: chat.turns.firstOrNull()?.prompt?.lineSequence()?.firstOrNull { it.isNotBlank() }?.trim()?.let { if (it.length > 80) it.take(80) + "…" else it }
        ?: newChat
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
                    if (onBack != null) IconButton(onClick = onBack) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
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
                                    starError = context.describe(e)
                                }
                            }
                        }) {
                            if (starred) {
                                Icon(Icons.Filled.Star, stringResource(R.string.unstar), tint = CedarColors.yellow)
                            } else {
                                Icon(Icons.Outlined.StarBorder, stringResource(R.string.star))
                            }
                        }
                    }
                    if (conversationId != null && chat.client != null) {
                        IconButton(onClick = { showingFiles = true }) { Icon(Icons.Outlined.Folder, stringResource(R.string.files)) }
                    }
                    if (webURL != null) {
                        IconButton(onClick = { openUrl(context, webURL) }) { Icon(Icons.Outlined.OpenInBrowser, stringResource(R.string.open_on_the_web)) }
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
    ErrorDialog(stringResource(R.string.could_not_change_star), starError) { starError = null }
    val client = chat.client
    if (showingFiles && conversationId != null && client != null) {
        WorkspaceFilesDialog(client, conversationId) { showingFiles = false }
    }
}

@Composable
private fun Greeting() {
    Column(Modifier.fillMaxWidth().padding(top = 72.dp, start = 16.dp, end = 16.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        CedarBadge()
        Spacer(Modifier.size(20.dp))
        Text(
            stringResource(R.string.greeting_title),
            style = MaterialTheme.typography.titleLarge,
            fontWeight = FontWeight.SemiBold,
            textAlign = TextAlign.Center,
        )
        Spacer(Modifier.size(8.dp))
        Text(
            stringResource(R.string.greeting_text),
            style = MaterialTheme.typography.bodyMedium,
            color = CedarColors.secondaryText,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 340.dp),
        )
    }
}

@Composable
fun UserBubble(text: String, attachments: List<AttachmentInfo> = emptyList()) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
        Spacer(Modifier.size(48.dp))
        Column(
            Modifier
                .clip(RoundedCornerShape(topStart = 20.dp, topEnd = 20.dp, bottomEnd = 6.dp, bottomStart = 20.dp))
                .background(MaterialTheme.colorScheme.primary)
                .padding(horizontal = 15.dp, vertical = 10.dp),
            horizontalAlignment = Alignment.End,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            SelectionContainer { Text(text, color = MaterialTheme.colorScheme.onPrimary) }
            if (attachments.isNotEmpty()) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    attachments.forEach { file ->
                        Row(
                            Modifier.clip(RoundedCornerShape(50)).background(Color.White.copy(alpha = 0.18f)).padding(horizontal = 8.dp, vertical = 4.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Icon(
                                if (file.mediaType == OutgoingAttachment.PDF) Icons.Outlined.Description else Icons.Outlined.Image, null, Modifier.size(14.dp),
                                tint = MaterialTheme.colorScheme.onPrimary,
                            )
                            Spacer(Modifier.size(4.dp))
                            Text(file.name, style = MaterialTheme.typography.labelSmall, maxLines = 1, color = MaterialTheme.colorScheme.onPrimary)
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
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(14.dp))
            .background(CedarColors.red.copy(alpha = 0.1f))
            .border(1.dp, CedarColors.red.copy(alpha = 0.25f), RoundedCornerShape(14.dp))
            .padding(12.dp),
        verticalAlignment = Alignment.Top,
    ) {
        Icon(Icons.Outlined.WarningAmber, null, Modifier.size(18.dp), tint = CedarColors.red)
        Spacer(Modifier.size(8.dp))
        Text(message, Modifier.weight(1f), style = MaterialTheme.typography.bodySmall, color = CedarColors.red)
        IconButton(onClick = dismiss, modifier = Modifier.size(24.dp)) { Icon(Icons.Filled.Close, stringResource(R.string.dismiss), tint = CedarColors.red) }
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
    Surface(color = MaterialTheme.colorScheme.background) {
        Column(
            Modifier.fillMaxWidth().navigationBarsPadding().imePadding().padding(start = 12.dp, end = 12.dp, top = 4.dp, bottom = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            if (chat.attachments.isNotEmpty()) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    chat.attachments.forEach { file -> AttachmentChip(file) { chat.removeAttachment(file) } }
                }
            }
            // A floating field: attach, the text, and send in one rounded card.
            Row(
                Modifier.fillMaxWidth().surfaceCard(radius = 24.dp).padding(5.dp),
                verticalAlignment = Alignment.Bottom,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Box {
                    FilledIconButton(
                        onClick = { menu = true },
                        enabled = !chat.isActive && !chat.isAddingAttachments && chat.attachments.size < OutgoingAttachment.MAX_FILES,
                        modifier = Modifier.size(36.dp),
                        colors = IconButtonDefaults.filledIconButtonColors(
                            containerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                            contentColor = CedarColors.secondaryText,
                            disabledContainerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                        ),
                    ) {
                        if (chat.isAddingAttachments) {
                            CircularProgressIndicator(Modifier.size(18.dp), strokeWidth = 2.dp)
                        } else {
                            Icon(
                                Icons.Filled.Add,
                                stringResource(if (chat.acceptsImages) R.string.attach_images_or_pdfs else R.string.attach_pdfs),
                            )
                        }
                    }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        // Photos only for a model that takes images; PDFs for any.
                        if (chat.acceptsImages) {
                            DropdownMenuItem(
                                text = { Text(stringResource(R.string.photos)) },
                                leadingIcon = { Icon(Icons.Outlined.PhotoLibrary, null) },
                                onClick = {
                                    menu = false
                                    photos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                                },
                            )
                        }
                        DropdownMenuItem(
                            text = { Text(stringResource(if (chat.acceptsImages) R.string.choose_files else R.string.choose_pdfs)) },
                            leadingIcon = { Icon(Icons.Outlined.Folder, null) },
                            onClick = {
                                menu = false
                                files.launch(if (chat.acceptsImages) arrayOf(OutgoingAttachment.PDF, "image/*") else arrayOf(OutgoingAttachment.PDF))
                            },
                        )
                    }
                }
                val placeholder = stringResource(if (chat.openQuestions == null) R.string.message_placeholder else R.string.answer_placeholder)
                val textStyle = MaterialTheme.typography.bodyLarge
                BasicTextField(
                    value = chat.draft,
                    onValueChange = { chat.draft = it },
                    modifier = Modifier.weight(1f).padding(vertical = 7.dp),
                    textStyle = textStyle.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                    maxLines = 8,
                    decorationBox = { field ->
                        Box {
                            if (chat.draft.isEmpty()) Text(placeholder, style = textStyle, color = CedarColors.secondaryText, maxLines = 1)
                            field()
                        }
                    },
                )
                if (chat.isActive) {
                    FilledIconButton(
                        onClick = { chat.stop() },
                        enabled = chat.pendingPrompt == null,
                        modifier = Modifier.size(36.dp),
                        colors = IconButtonDefaults.filledIconButtonColors(
                            containerColor = MaterialTheme.colorScheme.onSurface,
                            contentColor = MaterialTheme.colorScheme.background,
                        ),
                    ) {
                        Icon(Icons.Filled.Stop, stringResource(R.string.stop), Modifier.size(18.dp))
                    }
                } else {
                    FilledIconButton(
                        onClick = { chat.send() },
                        enabled = chat.canSend,
                        modifier = Modifier.size(36.dp),
                        colors = IconButtonDefaults.filledIconButtonColors(
                            disabledContainerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                            disabledContentColor = CedarColors.secondaryText,
                        ),
                    ) {
                        Icon(Icons.Filled.ArrowUpward, stringResource(R.string.send), Modifier.size(20.dp))
                    }
                }
            }
        }
    }
}

/** A file picked for the next message, with a thumbnail for images and a remove button. */
@Composable
private fun AttachmentChip(file: OutgoingAttachment, remove: () -> Unit) {
    val context = LocalContext.current
    var thumbnail by remember(file.id) { mutableStateOf<ImageBitmap?>(null) }
    LaunchedEffect(file.id) {
        if (file.isImage) {
            thumbnail = withContext(Dispatchers.Default) {
                val options = BitmapFactory.Options().apply { inSampleSize = 8 }
                BitmapFactory.decodeByteArray(file.data, 0, file.data.size, options)?.asImageBitmap()
            }
        }
    }
    Row(
        Modifier.surfaceCard(radius = 50.dp).padding(4.dp),
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
            Text(context.fileSize(file.data.size.toLong()), style = MaterialTheme.typography.labelSmall, color = CedarColors.secondaryText)
        }
        IconButton(onClick = remove, modifier = Modifier.size(28.dp)) {
            Icon(Icons.Outlined.Cancel, stringResource(R.string.remove_file, file.name), tint = CedarColors.secondaryText)
        }
    }
}
