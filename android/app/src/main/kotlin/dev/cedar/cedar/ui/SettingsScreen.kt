package dev.cedar.cedar.ui

import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.compose.foundation.background
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
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.KeyboardArrowRight
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.ArrowDropDown
import androidx.compose.material.icons.outlined.AutoAwesome
import androidx.compose.material.icons.outlined.BookmarkBorder
import androidx.compose.material.icons.outlined.TableChart
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Error
import androidx.compose.material.icons.outlined.Language
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.NotificationsOff
import androidx.compose.material.icons.outlined.OpenInBrowser
import androidx.compose.material.icons.outlined.QrCodeScanner
import androidx.compose.material.icons.outlined.Visibility
import androidx.compose.material.icons.outlined.VisibilityOff
import androidx.compose.material.icons.outlined.VolunteerActivism
import androidx.compose.material3.AlertDialog
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import androidx.core.app.NotificationManagerCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import dev.cedar.cedar.AppModel
import dev.cedar.cedar.BuildConfig
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
    data class Ok(val modelCount: Int) : Check
    data class Failed(val message: String) : Check
}

/**
 * Settings (iOS/Cedar/Features/Settings/SettingsView.swift). The server, the AI model and plan
 * approval are a draft that Save applies; the language, Memory, Contribute and Notifications apply
 * at once and say so. Closing asks before dropping edits.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SettingsScreen(app: AppModel, onAllowNotifications: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val focus = LocalFocusManager.current
    var draft by remember { mutableStateOf(app.settingsDraft ?: app.settings) }
    SideEffect { app.settingsDraft = draft }
    var check by remember { mutableStateOf<Check>(Check.Idle) }
    var models by remember { mutableStateOf(app.models) }
    var showToken by remember { mutableStateOf(false) }
    var scanError by remember { mutableStateOf<String?>(null) }
    var testStatus by remember { mutableStateOf<String?>(null) }
    var confirmingDiscard by remember { mutableStateOf(false) }
    val tokenField = remember { FocusRequester() }

    val hasChanges = draft != app.settings
    val canSave = draft.isConfigured && draft.urlProblem() == null && hasChanges

    fun close() {
        if (hasChanges) confirmingDiscard = true else app.showingSettings = false
    }

    fun testConnection(settings: ServerSettings) {
        focus.clearFocus()
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
                Check.Ok(models.size)
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

    // Back over unsaved edits asks first (the root's handler closes the screen otherwise).
    BackHandler(enabled = hasChanges && app.settings.isConfigured) { confirmingDiscard = true }

    // For the count on the Memory row.
    LaunchedEffect(app.serverHasMemory) { if (app.serverHasMemory) app.refreshMemory() }
    // And on the Records row.
    LaunchedEffect(app.serverHasRecords) { if (app.serverHasRecords) app.refreshRecords() }
    // For the On/Off on the Contribute row.
    LaunchedEffect(app.serverHasContributions) { if (app.serverHasContributions) app.refreshContributions() }
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
                        IconButton(onClick = ::close) { Icon(Icons.Filled.Close, stringResource(R.string.cancel)) }
                    }
                },
                actions = {
                    TextButton(
                        onClick = {
                            focus.clearFocus()
                            app.updateSettings(draft)
                            app.showingSettings = false
                            onAllowNotifications()
                        },
                        enabled = canSave,
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
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false, imeAction = ImeAction.Next),
                    keyboardActions = KeyboardActions(onNext = { tokenField.requestFocus() }),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = draft.accessToken,
                    onValueChange = { draft = draft.copy(accessToken = it); check = Check.Idle },
                    label = { Text(stringResource(R.string.access_token)) },
                    placeholder = { Text(stringResource(R.string.optional)) },
                    singleLine = true,
                    visualTransformation = if (showToken) VisualTransformation.None else PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false, imeAction = ImeAction.Go),
                    keyboardActions = KeyboardActions(onGo = { testConnection(draft) }),
                    trailingIcon = {
                        IconButton(onClick = { showToken = !showToken }) {
                            Icon(
                                if (showToken) Icons.Outlined.VisibilityOff else Icons.Outlined.Visibility,
                                stringResource(if (showToken) R.string.hide_token else R.string.show_token),
                            )
                        }
                    },
                    modifier = Modifier.fillMaxWidth().focusRequester(tokenField),
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlinedButton(onClick = {
                        focus.clearFocus()
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
                    if (check == Check.Checking) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                }
                // What the last test found.
                when (val c = check) {
                    is Check.Ok -> StatusLine(
                        Icons.Outlined.CheckCircle,
                        if (c.modelCount == 0) stringResource(R.string.connected_plain) else plural(R.plurals.connected_models, c.modelCount, c.modelCount),
                        CedarColors.green,
                    )
                    is Check.Failed -> StatusLine(Icons.Outlined.Error, c.message, CedarColors.red)
                    else -> {}
                }
                Footnote(stringResource(R.string.server_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.section_ai_model))
                val selected = models.firstOrNull { it.id == draft.model }
                val serverDefault = stringResource(R.string.server_default)
                Picker(
                    label = stringResource(R.string.ai_model),
                    // The saved pick, kept until the list is loaded or it is changed.
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
                        label = stringResource(R.string.reasoning_effort),
                        value = draft.reasoning.replaceFirstChar { it.uppercase() }.ifEmpty { default },
                        options = listOf("" to default) + levels.map { it to it.replaceFirstChar { c -> c.uppercase() } },
                    ) { draft = draft.copy(reasoning = it) }
                }
                Footnote(stringResource(if (models.isEmpty()) R.string.models_footnote_empty else R.string.models_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.section_plan_approval))
                val planModes = PLAN_MODES.map { (mode, english) -> mode to (planModeLabel(mode)?.let { stringResource(it) } ?: english) }
                Picker(
                    label = stringResource(R.string.show_plan_first),
                    value = planModes.firstOrNull { it.first == draft.plan }?.second ?: draft.plan,
                    options = planModes,
                ) { draft = draft.copy(plan = it) }
                Footnote(stringResource(R.string.plans_footnote))

                if (app.serverHasMemory) {
                    HorizontalDivider()
                    Section(stringResource(R.string.memory))
                    // Opens its own screen; deletions there apply at once, not on Save.
                    SettingsRow(
                        stringResource(R.string.memory_settings_row), Icons.Outlined.BookmarkBorder, MaterialTheme.colorScheme.primary,
                        value = if (app.memoryLoaded && app.memorySupported) plural(R.plurals.memory_count, app.memoryItems.size, app.memoryItems.size) else null,
                    ) { app.showingMemory = true }
                    Footnote(stringResource(R.string.memory_settings_footnote))
                }

                if (app.serverHasRecords) {
                    HorizontalDivider()
                    Section(stringResource(R.string.records))
                    // Opens its own screen; deletions there apply at once, not on Save.
                    SettingsRow(
                        stringResource(R.string.records_settings_row), Icons.Outlined.TableChart, MaterialTheme.colorScheme.primary,
                        value = if (app.recordsLoaded && app.recordsSupported) plural(R.plurals.records_count, app.recordSets.size, app.recordSets.size) else null,
                    ) { app.showingRecords = true }
                    Footnote(stringResource(R.string.records_settings_footnote))
                }

                if (app.serverHasContributions) {
                    HorizontalDivider()
                    Section(stringResource(R.string.contribute))
                    // Its own screen; its choices save at once, not on Save.
                    SettingsRow(
                        stringResource(R.string.contribute_settings_row), Icons.Outlined.VolunteerActivism, CedarColors.clay,
                        value = app.contributionSettings?.let { stringResource(if (it.enabled) R.string.contribute_on else R.string.contribute_off) },
                    ) { app.showingContribute = true }
                    Footnote(stringResource(R.string.contribute_settings_footnote))
                }

                HorizontalDivider()
                Notifications(app, onAllowNotifications, testStatus) { testStatus = it }

                HorizontalDivider()
                Section(stringResource(R.string.section_app))
                // Applies at once, unlike the rest of the screen (the app is shown again in the language).
                val language = app.language.orEmpty()
                val automatic = stringResource(R.string.language_automatic)
                Picker(
                    label = stringResource(R.string.language),
                    value = AppLocales.option(language)?.label ?: automatic,
                    options = listOf("" to automatic) + AppLocales.OPTIONS.map { it.tag to it.label },
                    icon = Icons.Outlined.Language,
                    tint = Color(0xFF3B7DD8),
                ) {
                    // The screen is shown again in the new language; what was typed carries over (settingsDraft).
                    if (it != language) app.chooseLanguage(it)
                }
                Footnote(stringResource(R.string.language_footnote))

                HorizontalDivider()
                Section(stringResource(R.string.section_about))
                Row(Modifier.fillMaxWidth().padding(vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(stringResource(R.string.version), Modifier.weight(1f))
                    Text("${BuildConfig.VERSION_NAME} (${BuildConfig.VERSION_CODE})", color = CedarColors.secondaryText)
                }
                app.client?.let { client ->
                    SettingsRow(stringResource(R.string.open_cedar_on_web), Icons.Outlined.OpenInBrowser, MaterialTheme.colorScheme.primary, chevron = false) {
                        openUrl(context, client.baseURL.toString())
                    }
                }
                SettingsRow(stringResource(R.string.see_what_cedar_can_do), Icons.Outlined.AutoAwesome, Color(0xFF8E5BD6)) { app.showingTour = true }
            }
        }
    }
    if (confirmingDiscard) {
        AlertDialog(
            onDismissRequest = { confirmingDiscard = false },
            title = { Text(stringResource(R.string.discard_changes_title)) },
            text = { Text(stringResource(R.string.discard_changes_text)) },
            confirmButton = {
                TextButton(onClick = {
                    confirmingDiscard = false
                    app.showingSettings = false
                }) { Text(stringResource(R.string.discard_changes), color = CedarColors.red) }
            },
            dismissButton = { TextButton(onClick = { confirmingDiscard = false }) { Text(stringResource(R.string.keep_editing)) } },
        )
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
    // Whether this phone may show notifications, read again when the app comes back from the system's settings.
    var allowed by remember { mutableStateOf(NotificationManagerCompat.from(context).areNotificationsEnabled()) }
    LifecycleResumeEffect(Unit) {
        allowed = NotificationManagerCompat.from(context).areNotificationsEnabled()
        onPauseOrDispose {}
    }

    Section(stringResource(R.string.section_notifications))
    SettingsRow(
        stringResource(R.string.section_notifications),
        if (allowed) Icons.Outlined.Notifications else Icons.Outlined.NotificationsOff,
        CedarColors.orange,
        value = stringResource(if (allowed) R.string.notifications_on else R.string.notifications_off),
        chevron = false,
        onClick = null,
    )
    if (!allowed) {
        Footnote(stringResource(R.string.notifications_off_footnote))
    }
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
        // Allowed: the phone registers itself on Save and at launch, so only the test is left to offer.
        if (!allowed) {
            Button(onClick = {
                // The system's page for this app's notifications; the permission prompt, where it has never been shown.
                val intent = Intent(Settings.ACTION_APP_NOTIFICATION_SETTINGS).putExtra(Settings.EXTRA_APP_PACKAGE, context.packageName)
                try {
                    context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                } catch (_: Exception) {
                    onAllowNotifications()
                }
            }) { Text(stringResource(R.string.allow_in_settings_app)) }
        }
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
            enabled = app.settings.isConfigured && allowed,
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

/** A line under the connection buttons: what the last test found. */
@Composable
private fun StatusLine(icon: ImageVector, text: String, color: Color) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        Icon(icon, null, Modifier.size(16.dp), tint = color)
        Text(text, style = MaterialTheme.typography.bodySmall, color = color)
    }
}

/** The small tinted tile in front of a settings row, as the system's Settings app draws them. */
@Composable
fun SettingsIcon(icon: ImageVector, tint: Color = MaterialTheme.colorScheme.primary) {
    Box(
        Modifier.size(30.dp).clip(RoundedCornerShape(8.dp)).background(tint.copy(alpha = 0.14f)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, null, Modifier.size(17.dp), tint = tint)
    }
}

/** A settings row: a tinted icon tile, the title, an optional value on the trailing side, and a chevron when it opens something. */
@Composable
private fun SettingsRow(
    title: String,
    icon: ImageVector,
    tint: Color,
    value: String? = null,
    chevron: Boolean = true,
    onClick: (() -> Unit)?,
) {
    Row(
        Modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        SettingsIcon(icon, tint)
        Text(title, Modifier.weight(1f))
        value?.let { Text(it, color = CedarColors.secondaryText) }
        if (chevron && onClick != null) Icon(Icons.AutoMirrored.Outlined.KeyboardArrowRight, null, tint = CedarColors.secondaryText)
    }
}

/** A labelled value that opens a menu of choices. */
@Composable
private fun Picker(
    label: String,
    value: String,
    options: List<Pair<String, String>>,
    icon: ImageVector? = null,
    tint: Color = MaterialTheme.colorScheme.primary,
    onPick: (String) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        Row(
            Modifier.fillMaxWidth().clickable { open = true }.padding(vertical = if (icon != null) 8.dp else 10.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            if (icon != null) SettingsIcon(icon, tint)
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
