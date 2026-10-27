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
import androidx.compose.runtime.SideEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.R
import dev.cedar.cedar.push.Transport
import dev.cedar.core.AppLocales
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

/** Server, model, language and notifications (iOS/Cedar/Features/Settings/SettingsView.swift). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(app: AppModel, onAllowNotifications: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf(app.settingsDraft ?: app.settings) }
    SideEffect { app.settingsDraft = draft }
    var check by remember { mutableStateOf<Check>(Check.Idle) }
    var models by remember { mutableStateOf(app.models) }
    var showToken by remember { mutableStateOf(false) }
    var scanError by remember { mutableStateOf<String?>(null) }
    var testStatus by remember { mutableStateOf<String?>(null) }

    fun testConnection(settings: ServerSettings) {
        settings.urlIssue()?.let {
            check = Check.Failed(context.urlProblem(it))
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
                Check.Failed(context.describe(e))
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
        if (code != null) take(code) else scanError = context.getString(R.string.scan_not_cedar)
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text(stringResource(R.string.settings)) },
                navigationIcon = {
                    if (app.settings.isConfigured) {
                        IconButton(onClick = { app.showingSettings = false }) { Icon(Icons.Filled.Close, stringResource(R.string.cancel)) }
                    }
                },
                actions = {
                    TextButton(
                        onClick = {
                            app.updateSettings(draft)
                            app.showingSettings = false
                            onAllowNotifications()
                        },
                        enabled = draft.isConfigured && draft.urlProblem() == null,
                    ) { Text(stringResource(R.string.save)) }
                },
            )
        },
    ) { padding ->
        Box(Modifier.padding(padding).fillMaxSize(), contentAlignment = Alignment.TopCenter) {
            Column(
                Modifier.widthIn(max = 640.dp).fillMaxWidth().verticalScroll(rememberScrollState()).imePadding().padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Section(stringResource(R.string.section_server))
                OutlinedTextField(
                    value = draft.serverURL,
                    onValueChange = { draft = draft.copy(serverURL = it); check = Check.Idle },
                    label = { Text(stringResource(R.string.server_url)) },
                    placeholder = { Text(stringResource(R.string.server_url_placeholder)) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = draft.accessToken,
                    onValueChange = { draft = draft.copy(accessToken = it); check = Check.Idle },
                    label = { Text(stringResource(R.string.access_token_label)) },
                    singleLine = true,
                    visualTransformation = if (showToken) VisualTransformation.None else PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                    trailingIcon = {
                        IconButton(onClick = { showToken = !showToken }) {
                            Icon(
                                if (showToken) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility,
                                stringResource(if (showToken) R.string.hide_token else R.string.show_token),
                            )
                        }
                    },
                    modifier = Modifier.fillMaxWidth(),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedButton(onClick = {
                        scan.launch(
                            ScanOptions()
                                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                .setPrompt(context.getString(R.string.scan_prompt))
                                .setBeepEnabled(false)
                                .setOrientationLocked(false),
                        )
                    }) {
                        Icon(Icons.Outlined.QrCodeScanner, null, Modifier.size(18.dp))
                        Spacer(Modifier.width(6.dp))
                        Text(stringResource(R.string.scan_qr))
                    }
                    Button(onClick = { testConnection(draft) }, enabled = draft.isConfigured && check != Check.Checking) {
                        Text(stringResource(R.string.test_connection))
                    }
                    when (check) {
                        Check.Checking -> CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                        Check.Ok -> Icon(Icons.Outlined.CheckCircle, stringResource(R.string.connected), tint = CedarColors.green)
                        is Check.Failed -> Icon(Icons.Outlined.Error, stringResource(R.string.failed), tint = CedarColors.red)
                        Check.Idle -> {}
                    }
                }
                (check as? Check.Failed)?.let { Text(it.message, style = MaterialTheme.typography.bodySmall, color = CedarColors.red) }
                Footnote(stringResource(R.string.server_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.language))
                // Applies at once, unlike the rest of the screen (the app is shown again in the language).
                val language = app.language.orEmpty()
                val automatic = stringResource(R.string.language_automatic)
                Picker(
                    label = stringResource(R.string.language),
                    value = AppLocales.option(language)?.label ?: automatic,
                    options = listOf("" to automatic) + AppLocales.OPTIONS.map { it.tag to it.label },
                ) {
                    // The screen is shown again in the new language; what was typed carries over (settingsDraft).
                    if (it != language) app.chooseLanguage(it)
                }
                Footnote(stringResource(R.string.language_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.model))
                val selected = models.firstOrNull { it.id == draft.model }
                val serverDefault = stringResource(R.string.server_default)
                Picker(
                    label = stringResource(R.string.model),
                    value = selected?.label ?: if (draft.model.isEmpty()) serverDefault else draft.model,
                    options = listOf("" to serverDefault) + models.map { it.id to it.label },
                ) { id ->
                    // A level the new model doesn't offer is dropped.
                    val levels = models.firstOrNull { it.id == id }?.reasoningLevels
                    draft = draft.copy(model = id, reasoning = if (levels != null && draft.reasoning !in levels) "" else draft.reasoning)
                }
                val levels = selected?.reasoningLevels.orEmpty()
                if (levels.isNotEmpty()) {
                    // The levels are the server's words (low, medium, high).
                    val default = stringResource(R.string.reasoning_default)
                    Picker(
                        label = stringResource(R.string.reasoning),
                        value = draft.reasoning.replaceFirstChar { it.uppercase() }.ifEmpty { default },
                        options = listOf("" to default) + levels.map { it to it.replaceFirstChar { c -> c.uppercase() } },
                    ) { draft = draft.copy(reasoning = it) }
                }
                Footnote(stringResource(if (models.isEmpty()) R.string.models_footnote_empty else R.string.models_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.section_plans))
                val planModes = PLAN_MODES.map { (mode, english) -> mode to (planModeLabel(mode)?.let { stringResource(it) } ?: english) }
                Picker(
                    label = stringResource(R.string.plan),
                    value = planModes.firstOrNull { it.first == draft.plan }?.second ?: draft.plan,
                    options = planModes,
                ) { draft = draft.copy(plan = it) }
                Footnote(stringResource(R.string.plans_footnote))

                HorizontalDivider()
                Notifications(app, onAllowNotifications, testStatus) { testStatus = it }
            }
        }
    }
    ErrorDialog(stringResource(R.string.not_a_cedar_code), scanError) { scanError = null }
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

    Section(stringResource(R.string.section_notifications))
    Text(stringResource(R.string.delivered_by), style = MaterialTheme.typography.labelLarge)
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
                    stringResource(
                        when (option) {
                            Transport.FCM -> if (fcm) R.string.fcm_available else R.string.fcm_unavailable
                            Transport.UNIFIED_PUSH -> R.string.unifiedpush_description
                        },
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = CedarColors.secondaryText,
                )
            }
        }
    }
    if (transport == Transport.UNIFIED_PUSH) {
        if (distributors.isEmpty()) {
            Footnote(stringResource(R.string.no_unifiedpush))
            OutlinedButton(onClick = { openUrl(context, "https://ntfy.sh/docs/subscribe/phone/") }) { Text(stringResource(R.string.get_ntfy)) }
        } else if (distributors.size > 1 || push.savedDistributor == null) {
            Picker(
                label = stringResource(R.string.unifiedpush_app),
                value = push.savedDistributor ?: stringResource(R.string.choose),
                options = distributors.map { it to it },
            ) { push.useDistributor(it) }
        }
    }
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Button(onClick = onAllowNotifications, enabled = app.settings.isConfigured) { Text(stringResource(R.string.allow_notifications)) }
        OutlinedButton(
            onClick = {
                setTestStatus(context.getString(R.string.sending))
                scope.launch {
                    setTestStatus(
                        try {
                            push.sendTest()
                        } catch (e: kotlinx.coroutines.CancellationException) {
                            throw e
                        } catch (e: Exception) {
                            context.describe(e)
                        },
                    )
                }
            },
            enabled = app.settings.isConfigured,
        ) { Text(stringResource(R.string.send_test)) }
    }
    push.status?.let { Footnote(it) }
    testStatus?.let { Footnote(it) }
    Footnote(stringResource(R.string.notifications_footnote))
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
