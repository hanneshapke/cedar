package dev.cedar.cedar.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.ui.unit.sp
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.EditNote
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.key
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R

/**
 * The list and the open chat: side by side on a tablet, a foldable or a phone in landscape (the
 * iPad's split view), one at a time on a phone, where Back returns to the list. Onboarding comes
 * first: the language, then the server (Settings).
 */
@Composable
fun RootScreen(app: AppModel, onAllowNotifications: () -> Unit) {
    // Settings closed: its next opening starts from the saved settings.
    LaunchedEffect(app.showingSettings) { if (!app.showingSettings) app.settingsDraft = null }
    Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
        if (app.language == null) {
            LanguageScreen(app)
        } else if (app.showingMemory) {
            BackHandler { app.showingMemory = false }
            MemoryScreen(app)
        } else if (app.showingSettings) {
            BackHandler(enabled = app.settings.isConfigured) { app.showingSettings = false }
            SettingsScreen(app, onAllowNotifications)
        } else BoxWithConstraints(Modifier.fillMaxSize()) {
            val wide = maxWidth >= 720.dp
            if (wide) {
                Row(Modifier.fillMaxSize()) {
                    Box(Modifier.width(360.dp).fillMaxHeight()) { ConversationListScreen(app) }
                    VerticalDivider()
                    Box(Modifier.weight(1f).fillMaxHeight()) { Detail(app, showBack = false) }
                }
            } else {
                val selection = app.selection
                if (selection != null) {
                    BackHandler { app.selection = null }
                    Detail(app, showBack = true)
                } else {
                    ConversationListScreen(app)
                }
            }
        }
    }
}

@Composable
private fun Detail(app: AppModel, showBack: Boolean) {
    val selection = app.selection
    if (selection == null) {
        Placeholder(onNewChat = { app.newChat() })
        return
    }
    val chat = app.chat(selection)
    // Keyed by the chat, not the selection: a draft keeps its screen when it becomes a conversation.
    key(chat.key) {
        ChatScreen(app, chat, onBack = if (showBack) ({ app.selection = null }) else null)
    }
}

@Composable
private fun Placeholder(onNewChat: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(32.dp),
        verticalArrangement = androidx.compose.foundation.layout.Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        CedarBadge(88.dp)
        Spacer(Modifier.height(18.dp))
        CedarWordmark(28.sp)
        Spacer(Modifier.height(10.dp))
        Text(
            stringResource(R.string.placeholder_text),
            style = MaterialTheme.typography.bodyMedium,
            color = CedarColors.secondaryText,
            textAlign = TextAlign.Center,
            modifier = Modifier.widthIn(max = 360.dp),
        )
        Spacer(Modifier.height(22.dp))
        ProminentButton(onClick = onNewChat, modifier = Modifier.widthIn(min = 200.dp)) {
            Icon(Icons.Outlined.EditNote, null, Modifier.size(20.dp))
            Spacer(Modifier.width(8.dp))
            Text(stringResource(R.string.new_chat))
        }
    }
}
