import SwiftUI
import UIKit
import UserNotifications

/// The settings sheet. The server, the model and the plan mode are a draft that Save applies;
/// the language, Memory, Contribute and Notifications apply at once and say so. Cancel asks
/// before dropping edits, and the sheet can't be swiped away over unsaved ones.
struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @Environment(\.openURL) private var openURL
    @Environment(\.scenePhase) private var scenePhase
    @State private var draft = ServerSettings(serverURL: "", accessToken: "", model: "", reasoning: "")
    @State private var check: CheckState = .idle
    @State private var models: [ModelInfo] = []
    @State private var scanning = false
    /// Set by a scan, so the connection is tested once the scanner is gone.
    @State private var scanned = false
    /// The draft was filled in; coming back from Memory doesn't reset what was typed.
    @State private var started = false
    @State private var showingToken = false
    @State private var confirmingDiscard = false
    @State private var showingTour = false
    /// Whether this phone may show notifications, once asked.
    @State private var notifications: UNAuthorizationStatus?
    @FocusState private var focus: Field?

    enum CheckState: Equatable {
        case idle, checking
        case ok(modelCount: Int)
        case failed(String)
    }

    private enum Field {
        case url, token
    }

    var body: some View {
        NavigationStack {
            Form {
                server
                model
                plans
                if app.serverHasMemory { memory }
                if app.serverHasContributions { contribute }
                notificationsSection
                language
                about
            }
            .scrollContentBackground(.hidden)
            .background(Theme.canvas)
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        if hasChanges {
                            confirmingDiscard = true
                        } else {
                            dismiss()
                        }
                    }
                    .disabled(!app.settings.isConfigured)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") { save() }
                        .disabled(!draft.isConfigured || !hasChanges)
                }
            }
            .confirmationDialog("Discard Changes?", isPresented: $confirmingDiscard, titleVisibility: .visible) {
                Button("Discard Changes", role: .destructive) { dismiss() }
                Button("Keep Editing", role: .cancel) {}
            } message: {
                Text("Your changes to the server, AI model and plan approval are not saved yet.")
            }
            .sheet(isPresented: $showingTour) {
                OnboardingTourView { showingTour = false }
            }
            .sheet(isPresented: $scanning, onDismiss: {
                if scanned {
                    scanned = false
                    Task { await testConnection() }
                }
            }) {
                ScanCodeView { code in
                    draft.serverURL = code.serverURL
                    draft.accessToken = code.accessToken
                    scanned = true
                }
            }
            .onAppear {
                guard !started else { return }
                started = true
                draft = app.settings
                models = app.models
            }
            .task { await readNotificationStatus() }
            // Back from the system's Settings app, where notifications may have been allowed.
            .onChange(of: scenePhase) { _, phase in
                if phase == .active { Task { await readNotificationStatus() } }
            }
            // For the On/Off on the Contribute row.
            .task(id: app.serverHasContributions) {
                if app.serverHasContributions { await app.refreshContributions() }
            }
            // For the count on the Memory row.
            .task(id: app.serverHasMemory) {
                if app.serverHasMemory { await app.refreshMemory() }
            }
            .onChange(of: draft.serverURL) { check = .idle }
            .onChange(of: draft.accessToken) { check = .idle }
            .onChange(of: draft.model) {
                // A level the new model doesn't offer is dropped; while the models aren't
                // loaded yet, the saved choice is kept.
                if let levels = selectedModel?.reasoningLevels, !draft.reasoning.isEmpty, !levels.contains(draft.reasoning) {
                    draft.reasoning = ""
                }
            }
        }
        .interactiveDismissDisabled(hasChanges || !app.settings.isConfigured)
    }

    // MARK: Sections

    private var server: some View {
        Section {
            VStack(alignment: .leading, spacing: 4) {
                Text("Server URL")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                TextField(ServerSettings.defaultServerURL, text: $draft.serverURL)
                    .keyboardType(.URL)
                    .textContentType(.URL)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.next)
                    .focused($focus, equals: .url)
                    .onSubmit { focus = .token }
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Access Token")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                HStack {
                    Group {
                        if showingToken {
                            TextField("Optional", text: $draft.accessToken)
                        } else {
                            SecureField("Optional", text: $draft.accessToken)
                        }
                    }
                    .textContentType(.password)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .submitLabel(.go)
                    .focused($focus, equals: .token)
                    .onSubmit { Task { await testConnection() } }
                    Button {
                        showingToken.toggle()
                    } label: {
                        Image(systemName: showingToken ? "eye.slash" : "eye")
                            .foregroundStyle(.secondary)
                    }
                    .buttonStyle(.borderless)
                    .accessibilityLabel(showingToken ? Text("Hide token") : Text("Show token"))
                }
            }
            Button {
                focus = nil
                scanning = true
            } label: {
                Label("Scan QR Code", systemImage: "qrcode.viewfinder")
            }
            Button {
                focus = nil
                Task { await testConnection() }
            } label: {
                HStack {
                    Label("Test Connection", systemImage: "antenna.radiowaves.left.and.right")
                    Spacer()
                    if check == .checking {
                        ProgressView()
                    }
                }
            }
            .disabled(!draft.isConfigured || check == .checking)
            connectionStatus
        } header: {
            Text("Server")
        } footer: {
            Text("The address where you open cedar-chat in a browser, and its access token if it has one. On a Mac, turn on iPhone & iPad sharing in the Cedar app and scan the code it shows. The token stays in this phone's Keychain.")
        }
    }

    /// What the last test found, under the Test Connection row.
    @ViewBuilder
    private var connectionStatus: some View {
        switch check {
        case .idle, .checking:
            EmptyView()
        case .ok(let modelCount):
            Label {
                Text(verbatim: modelCount == 0
                     ? L10n.string("Connected.")
                     : L10n.format(modelCount == 1 ? "Connected. %lld model available." : "Connected. %lld models available.", modelCount))
            } icon: {
                Image(systemName: "checkmark.circle.fill")
            }
            .font(.footnote)
            .foregroundStyle(Theme.green)
        case .failed(let message):
            Label {
                Text(message)
            } icon: {
                Image(systemName: "xmark.circle.fill")
            }
            .font(.footnote)
            .foregroundStyle(Theme.clay)
        }
    }

    private var model: some View {
        Section {
            Picker("AI Model", selection: $draft.model) {
                Text("Server default").tag("")
                ForEach(models) { model in
                    Text(model.label).tag(model.id)
                }
                // The saved pick, kept until the list is loaded or it is changed.
                if !draft.model.isEmpty, !models.contains(where: { $0.id == draft.model }) {
                    Text(verbatim: draft.model).tag(draft.model)
                }
            }
            if let levels = selectedModel?.reasoningLevels, !levels.isEmpty {
                Picker("Reasoning Effort", selection: $draft.reasoning) {
                    Text("Default").tag("")
                    ForEach(levels, id: \.self) { level in
                        Text(level.capitalized).tag(level)
                    }
                }
            }
        } header: {
            Text("AI Model")
        } footer: {
            Text(models.isEmpty
                 ? "The AI models your server offers are listed once it is reachable."
                 : "Which AI answers your messages, and how hard it thinks before answering. Tools, the turn limit and the system prompt follow the server's Settings.")
        }
    }

    private var plans: some View {
        Section {
            Picker("Show a Plan First", selection: $draft.plan) {
                ForEach(ServerSettings.planModes) { mode in
                    Text(L10n.string(mode.label)).tag(mode.id)
                }
            }
        } header: {
            Text("Plan Approval")
        } footer: {
            Text("Before a bigger request, Cedar lists the steps it will take and waits for your go-ahead; you can stop it there. With \u{201C}When Cedar thinks it helps\u{201D}, it decides which requests need a plan, so quick questions get none.")
        }
    }

    private var memory: some View {
        Section {
            // Its own screen; deletions there apply at once, not on Save.
            NavigationLink {
                MemoryView()
            } label: {
                SettingsRow("What Cedar Remembers", systemImage: "bookmark", tint: Theme.green) {
                    if app.memoryLoaded && app.memorySupported {
                        Text(verbatim: L10n.format(app.memoryItems.count == 1 ? "%lld memory" : "%lld memories", app.memoryItems.count))
                    }
                }
            }
        } header: {
            Text("Memory")
        } footer: {
            Text("Facts Cedar keeps about you across conversations, like your name, preferences and projects. Deleting one applies from your next message.")
        }
    }

    private var contribute: some View {
        Section {
            // Its own screen; its choices save at once, not on Save.
            NavigationLink {
                ContributeSettingsView()
            } label: {
                SettingsRow("Contribute Conversations", systemImage: "arrow.up.heart", tint: Theme.clay) {
                    if let settings = app.contributionSettings {
                        Text(settings.enabled ? "On" : "Off")
                    }
                }
            }
        } header: {
            Text("Contribute")
        } footer: {
            Text("Give single conversations to a shared corpus for training open-weight agent models. Opt-in, one conversation at a time, after you read exactly what goes.")
        }
    }

    private var notificationsSection: some View {
        Section {
            switch notifications {
            case .authorized, .provisional, .ephemeral:
                SettingsRow("Notifications", systemImage: "bell.badge", tint: .orange) {
                    Text("Turned on")
                }
            case .denied:
                SettingsRow("Notifications", systemImage: "bell.slash", tint: .orange) {
                    Text("Turned off")
                }
                Button("Allow in the Settings App") {
                    if let url = URL(string: UIApplication.openSettingsURLString) {
                        openURL(url)
                    }
                }
            default:
                Button {
                    Task {
                        await PushRegistrar.shared?.requestAuthorization()
                        await readNotificationStatus()
                    }
                } label: {
                    SettingsRow("Allow Notifications", systemImage: "bell.badge", tint: .orange)
                }
            }
        } header: {
            Text("Notifications")
        } footer: {
            Text(notifications == .denied
                 ? "Notifications are turned off for Cedar in the Settings app. Turn them on there to hear when a response is ready."
                 : "The server sends a push when a response is ready or Cedar has a question. Set up iPhone push under Settings → Notifications in the web UI; this phone registers itself there.")
        }
    }

    private var language: some View {
        Section {
            // Applies at once, without Save.
            Picker(selection: languageTag) {
                Text("Automatic").tag("")
                ForEach(AppLanguage.all) { language in
                    Text(verbatim: language.name).tag(language.tag)
                }
            } label: {
                SettingsRow("Language", systemImage: "globe", tint: .blue)
            }
            .pickerStyle(.navigationLink)
        } header: {
            Text("App")
        } footer: {
            Text("Applies at once. Automatic follows this device's language. Scheduled runs and notifications use the language too.")
        }
    }

    private var about: some View {
        Section {
            LabeledContent("Version") {
                Text(verbatim: versionText)
            }
            if let client = app.client {
                Link(destination: client.baseURL) {
                    Label("Open Cedar on the Web", systemImage: "safari")
                }
            }
            Button {
                showingTour = true
            } label: {
                SettingsRow("See What Cedar Can Do", systemImage: "sparkles", tint: .purple)
            }
        } header: {
            Text("About")
        }
    }

    // MARK: Helpers

    /// Whether Save has anything to apply.
    private var hasChanges: Bool {
        draft != app.settings
    }

    /// The picked language's tag, or empty for automatic.
    private var languageTag: Binding<String> {
        Binding(
            get: { LanguageStore.shared.chosenTag ?? "" },
            set: { app.chooseLanguage($0.isEmpty ? nil : $0) }
        )
    }

    private var selectedModel: ModelInfo? {
        models.first { $0.id == draft.model }
    }

    private var versionText: String {
        let info = Bundle.main.infoDictionary
        let version = info?["CFBundleShortVersionString"] as? String ?? "?"
        let build = info?["CFBundleVersion"] as? String ?? "?"
        return "\(version) (\(build))"
    }

    private func save() {
        focus = nil
        app.updateSettings(draft)
        dismiss()
    }

    private func testConnection() async {
        check = .checking
        do {
            let client = try ChatClient(settings: draft)
            try await client.verify()
            models = (try? await client.models()) ?? []
            check = .ok(modelCount: models.count)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
        } catch {
            check = .failed(error.localizedDescription)
            UINotificationFeedbackGenerator().notificationOccurred(.error)
        }
    }

    private func readNotificationStatus() async {
        notifications = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }
}

/// A settings row as the system's Settings app draws them: a tinted icon tile, the title, and
/// an optional value on the trailing side.
struct SettingsRow<Value: View>: View {
    let title: LocalizedStringKey
    let systemImage: String
    let tint: Color
    @ViewBuilder let value: () -> Value

    init(_ title: LocalizedStringKey, systemImage: String, tint: Color, @ViewBuilder value: @escaping () -> Value) {
        self.title = title
        self.systemImage = systemImage
        self.tint = tint
        self.value = value
    }

    var body: some View {
        LabeledContent {
            value()
        } label: {
            Label {
                Text(title)
            } icon: {
                SettingsIcon(systemImage: systemImage, tint: tint)
            }
        }
    }
}

extension SettingsRow where Value == EmptyView {
    init(_ title: LocalizedStringKey, systemImage: String, tint: Color) {
        self.init(title, systemImage: systemImage, tint: tint) { EmptyView() }
    }
}

/// The small tinted tile in front of a settings row.
struct SettingsIcon: View {
    let systemImage: String
    var tint: Color = Theme.green

    var body: some View {
        Image(systemName: systemImage)
            .font(.system(size: 15, weight: .medium))
            .foregroundStyle(tint)
            .frame(width: 30, height: 30)
            .background(tint.opacity(0.14), in: RoundedRectangle(cornerRadius: 8, style: .continuous))
            .accessibilityHidden(true)
    }
}
