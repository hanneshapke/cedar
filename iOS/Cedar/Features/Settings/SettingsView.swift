import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var draft = ServerSettings(serverURL: "", accessToken: "", model: "", reasoning: "")
    @State private var check: CheckState = .idle
    @State private var models: [ModelInfo] = []
    @State private var pushStatus: String?
    @State private var scanning = false
    /// Set by a scan, so the connection is tested once the scanner is gone.
    @State private var scanned = false

    enum CheckState: Equatable {
        case idle, checking, ok
        case failed(String)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(ServerSettings.defaultServerURL, text: $draft.serverURL)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    SecureField("Access token (if the server has one)", text: $draft.accessToken)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button {
                        scanning = true
                    } label: {
                        Label("Scan QR Code", systemImage: "qrcode.viewfinder")
                    }
                } header: {
                    Text("Server")
                } footer: {
                    Text("The cedar-chat server's URL (where you open the web UI) and its CHAT_ACCESS_TOKEN, if it is set. Running Cedar on a Mac? Turn on iPhone & iPad sharing in the Mac app and scan the QR code it shows. The token is stored in the Keychain; cedar's own key stays on the server.")
                }

                Section {
                    Button {
                        Task { await testConnection() }
                    } label: {
                        HStack {
                            Text("Test Connection")
                            Spacer()
                            switch check {
                            case .idle: EmptyView()
                            case .checking: ProgressView()
                            case .ok: Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                            case .failed: Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
                            }
                        }
                    }
                    .disabled(!draft.isConfigured || check == .checking)
                    if case .failed(let message) = check {
                        Text(message)
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                }

                Section {
                    Picker("Model", selection: $draft.model) {
                        Text("Server default").tag("")
                        ForEach(models) { model in
                            Text(model.label).tag(model.id)
                        }
                    }
                    if let levels = selectedModel?.reasoningLevels, !levels.isEmpty {
                        Picker("Reasoning", selection: $draft.reasoning) {
                            Text("Default").tag("")
                            ForEach(levels, id: \.self) { level in
                                Text(level.capitalized).tag(level)
                            }
                        }
                    }
                } header: {
                    Text("Model")
                } footer: {
                    Text(models.isEmpty
                         ? "The models cedar has registered are listed once the server is reachable."
                         : "Used for your messages. Tools, the turn limit and the system prompt follow the server's Settings.")
                }

                Section {
                    Button("Allow Notifications") {
                        Task {
                            await PushRegistrar.shared?.requestAuthorization()
                            pushStatus = "If notifications are allowed, this phone is registered with the server the next time it starts a response."
                        }
                    }
                    if let pushStatus {
                        Text(pushStatus)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                } header: {
                    Text("Notifications")
                } footer: {
                    Text("Pushes come from the server: set up iPhone push under Settings → Notifications in the web UI, with your Apple team id and APNs key. This phone registers itself there.")
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                        .disabled(!app.settings.isConfigured)
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Save") {
                        app.updateSettings(draft)
                        dismiss()
                    }
                    .disabled(!draft.isConfigured)
                }
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
                draft = app.settings
                models = app.models
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
        .interactiveDismissDisabled(!app.settings.isConfigured)
    }

    private var selectedModel: ModelInfo? {
        models.first { $0.id == draft.model }
    }

    private func testConnection() async {
        check = .checking
        do {
            let client = try ChatClient(settings: draft)
            try await client.verify()
            models = (try? await client.models()) ?? []
            check = .ok
        } catch {
            check = .failed(error.localizedDescription)
        }
    }
}
