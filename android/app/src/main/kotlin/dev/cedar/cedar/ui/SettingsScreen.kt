package dev.cedar.cedar.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.ArrowDropDown
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Error
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.push.Transport
import dev.cedar.core.ChatClient
import dev.cedar.core.ConnectionCode
import dev.cedar.core.PLAN_MODES
import dev.cedar.core.ServerSettings
import kotlinx.coroutines.launch

private sealed interface Check {
    data object Idle : Check
    data object Checking : Check
    data object Ok : Check
    data class Failed(val message: String) : Check
}

/** Server, model and notifications (iOS/Cedar/Features/Settings/SettingsView.swift). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(app: AppModel, onAllowNotifications: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf(app.settings) }
    var check by remember { mutableStateOf<Check>(Check.Idle) }
    var models by remember { mutableStateOf(app.models) }
    var showToken by remember { mutableStateOf(false) }
    var scanError by remember { mutableStateOf<String?>(null) }
    var testStatus by remember { mutableStateOf<String?>(null) }

    fun testConnection(settings: ServerSettings) {
        settings.urlProblem()?.let {
            check = Check.Failed(it)
            return
        }
        check = Check.Checking
        scope.launch {
            check = try {
                val client = ChatClient.from(settings)
                client.verify()
                models = runCatching { client.models() }.getOrDefault(emptyList())
                Check.Ok
            } catch (e: kotlinx.coroutines.CancellationException) {
                throw e
            } catch (e: Exception) {
                Check.Failed(e.message ?: e.toString())
            }
        }
    }

    fun take(code: ConnectionCode) {
        draft = draft.copy(serverURL = code.serverURL, accessToken = code.accessToken)
        testConnection(draft)
    }

    // A cedar://connect link opened from another app (or the camera's QR reader).
    LaunchedEffect(app.pendingConnection) {
        app.pendingConnection?.let {
            app.pendingConnection = null
            take(it)
        }
    }

    val scan = rememberLauncherForActivityResult(ScanContract()) { result ->
        val contents = result.contents ?: return@rememberLauncherForActivityResult
        val code = ConnectionCode.parse(contents)
        if (code != null) take(code) else scanError = "That QR code isn't a Cedar connection code. Scan the one under Settings → Phone in the chat, in the desktop app, or from cedar-stack.sh --pair."
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Settings") },
                navigationIcon = {
                    if (app.settings.isConfigured) IconButton(onClick = { app.showingSettings = false }) { Icon(Icons.Filled.Close, "Cancel") }
                },
                actions = {
                    TextButton(
                        onClick = {
                            app.updateSettings(draft)
                            app.showingSettings = false
                            onAllowNotifications()
                        },
                        enabled = draft.isConfigured && draft.urlProblem() == null,
                    ) { Text("Save") }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 640.dp).fillMaxWidth().verticalScroll(rememberScrollState()).imePadding().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Section("Server")
                OutlinedTextField(
                    value = draft.serverURL,
                    onValueChange = { draft = draft.copy(serverURL = it); check = Check.Idle },
                    label = { Text("Server URL") },
                    placeholder = { Text("https://<machine>.<tailnet>.ts.net") },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = draft.accessToken,
                    onValueChange = { draft = draft.copy(accessToken = it); check = Check.Idle },
                    label = { Text("Access token (if the server has one)") },
                    singleLine = true,
                    visualTransformation = if (showToken) VisualTransformation.None else PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                    trailingIcon = {
                        IconButton(onClick = { showToken = !showToken }) {
                            Icon(if (showToken) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility, if (showToken) "Hide token" else "Show token")
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedButton(onClick = {
                        scan.launch(
                            ScanOptions()
                                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                .setPrompt("Scan the QR code under Settings → Phone in the chat")
                                .setBeepEnabled(false)
                                .setOrientationLocked(false),
                        )
                    }) {
                        Icon(Icons.Outlined.QrCodeScanner, null, Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text("Scan QR Code")
                    }
                    Button(onClick = { testConnection(draft) }, enabled = draft.isConfigured && check != Check.Checking) { Text("Test Connection") }
                    when (check) {
                        Check.Checking -> CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        Check.Ok -> Icon(Icons.Outlined.CheckCircle, "Connected", tint = CedarColors.green)
                        is Check.Failed -> Icon(Icons.Outlined.Error, "Failed", tint = CedarColors.red)
                        Check.Idle -> {}
                    }
                }
                (check as? Check.Failed)?.let { Text(it.message, style = MaterialTheme.typography.bodySmall, color = CedarColors.red) }
                Footnote(
                    "The cedar-chat server's address (where you open the web UI) and its CHAT_ACCESS_TOKEN, if it is set. The app " +
                        "connects over HTTPS only: serve the chat with tailscale serve, which the Cedar desktop app and " +
                        "scripts/cedar-stack.sh do for you, then scan the QR code under Settings → Phone in the chat. The token is " +
                        "stored encrypted on this phone; cedar's own key stays on the server.",
                )

                HorizontalDivider()
                Section("Model")
                val selected = models.firstOrNull { it.id == draft.model }
                Picker(
                    label = "Model",
                    value = selected?.label ?: if (draft.model.isEmpty()) "Server default" else draft.model,
                    options = listOf("" to "Server default") + models.map { it.id to it.label },
                ) { id ->
                    // A level the new model doesn't offer is dropped.
                    val levels = models.firstOrNull { it.id == id }?.reasoningLevels
                    draft = draft.copy(model = id, reasoning = if (levels != null && draft.reasoning !in levels) "" else draft.reasoning)
                }
                val levels = selected?.reasoningLevels.orEmpty()
                if (levels.isNotEmpty()) {
                    Picker(
                        label = "Reasoning",
                        value = draft.reasoning.ifEmpty { "Default" }.replaceFirstChar { it.uppercase() },
                        options = listOf("" to "Default") + levels.map { it to it.replaceFirstChar { c -> c.uppercase() } },
                    ) { draft = draft.copy(reasoning = it) }
                }
                Footnote(
                    if (models.isEmpty()) "The models cedar has registered are listed once the server is reachable."
                    else "Used for your messages. Tools, the turn limit and the system prompt follow the server's Settings.",
                )

                HorizontalDivider()
                Section("Plans")
                Picker(
                    label = "Plan",
                    value = PLAN_MODES.firstOrNull { it.first == draft.plan }?.second ?: draft.plan,
                    options = PLAN_MODES,
                ) { draft = draft.copy(plan = it) }
                Footnote(
                    "Cedar shows its steps and waits for your go-ahead; you can stop it there. With \"When Cedar thinks it " +
                        "helps\", it decides which requests need a plan, so quick questions get none.",
                )

                HorizontalDivider()
                Notifications(app, onAllowNotifications, testStatus) { testStatus = it }
            }
        }
    }
    ErrorDialog("Not a Cedar Code", scanError) { scanError = null }
}

@Composable
private fun Notifications(app: AppModel, onAllowNotifications: () -> Unit, testStatus: String?, setTestStatus: (String?) -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val push = app.push
    @Suppress("UNUSED_VARIABLE") val revision = push.revision
    val transport = push.transport
    val fcm = push.fcmAvailable
    val distributors = push.distributors

    Section("Notifications")
    Text("Delivered by", style = MaterialTheme.typography.labelLarge)
    Transport.entries.forEach { option ->
        val enabled = option != Transport.FCM || fcm
        Row(
            Modifier.fillMaxWidth().selectable(selected = transport == option, enabled = enabled) { push.switchTo(option) },
            verticalAlignment = Alignment.CenterVertically,
        ) {
            RadioButton(selected = transport == option, onClick = null, enabled = enabled)
            Spacer(Modifier.width(8.dp))
            Column {
                Text(option.label, color = if (enabled) MaterialTheme.colorScheme.onSurface else CedarColors.secondaryText)
                Text(
                    when (option) {
                        Transport.FCM -> if (fcm) "Through Google Play services." else "Needs Google Play services and a build with the Firebase project."
                        Transport.UNIFIED_PUSH -> "Through the ntfy app (ntfy.sh or your own server), without Google."
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = CedarColors.secondaryText,
                )
            }
        }
    }
    if (transport == Transport.UNIFIED_PUSH) {
        if (distributors.isEmpty()) {
            Footnote("No UnifiedPush app is installed. Install ntfy, from F-Droid or Google Play, then come back here.")
            OutlinedButton(onClick = { openUrl(context, "https://ntfy.sh/docs/subscribe/phone/") }) { Text("Get ntfy") }
        } else if (distributors.size > 1 || push.savedDistributor == null) {
            Picker(
                label = "UnifiedPush app",
                value = push.savedDistributor ?: "Choose…",
                options = distributors.map { it to it },
            ) { push.useDistributor(it) }
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = onAllowNotifications, enabled = app.settings.isConfigured) { Text("Allow Notifications") }
        OutlinedButton(
            onClick = {
                setTestStatus("Sending…")
                scope.launch {
                    setTestStatus(
                        try {
                            push.sendTest()
                        } catch (e: kotlinx.coroutines.CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            e.message
                        },
                    )
                }
            },
            enabled = app.settings.isConfigured,
        ) { Text("Send a Test") }
    }
    push.status?.let { Footnote(it) }
    testStatus?.let { Footnote(it) }
    Footnote(
        "Pushes come from the server: choose Android push under Settings → Notifications in the web UI. FCM needs the " +
            "Firebase service-account key there; ntfy needs nothing. This phone registers itself, and a push carries only " +
            "which conversation finished: the text is fetched from your server.",
    )
}

@Composable
private fun Section(title: String) {
    Text(title, style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.primary)
}

@Composable
private fun Footnote(text: String) {
    Text(text, style = MaterialTheme.typography.bodySmall, color = CedarColors.secondaryText)
}

/** A labelled value that opens a menu of choices. */
@Composable
private fun Picker(label: String, value: String, options: List<Pair<String, String>>, onPick: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier.fillMaxWidth().clickable { open = true }.padding(vertical = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(label, Modifier.weight(1f))
            Text(value, color = CedarColors.secondaryText)
            Icon(Icons.Outlined.ArrowDropDown, null, tint = CedarColors.secondaryText)
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            options.forEach { (id, text) ->
                DropdownMenuItem(text = { Text(text) }, onClick = {
                    open = false
                    onPick(id)
                })
            }
        }
    }
}
