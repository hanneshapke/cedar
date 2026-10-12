import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var draft = ServerSettings(serverURL: "", apiKey: "", model: "", systemPrompt: "")
    @State private var check: CheckState = .idle

    enum CheckState: Equatable {
        case idle, checking, ok
        case failed(String)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField("http://127.0.0.1:8080", text: $draft.serverURL)
                        .keyboardType(.URL)
                        .textContentType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    SecureField("API key", text: $draft.apiKey)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text("Server")
                } footer: {
                    Text("The cedar server's URL and one of its CEDAR_API_KEYS. The key is stored in the Keychain.")
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
                    TextField("Server default", text: $draft.model)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                } header: {
                    Text("Model")
                } footer: {
                    Text("Leave empty to use the server's LLM_MODEL. A model must be in the server's ALLOWED_MODELS if that is set.")
                }

                Section {
                    TextField("Server default", text: $draft.systemPrompt, axis: .vertical)
                        .lineLimit(3...8)
                } header: {
                    Text("System Prompt")
                } footer: {
                    Text("Used for new conversations. Follow-ups keep the prompt their conversation started with.")
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
            .onAppear { draft = app.settings }
            .onChange(of: draft.serverURL) { check = .idle }
            .onChange(of: draft.apiKey) { check = .idle }
        }
        .interactiveDismissDisabled(!app.settings.isConfigured)
    }

    private func testConnection() async {
        check = .checking
        do {
            try await CedarClient(settings: draft).verify()
            check = .ok
        } catch {
            check = .failed(error.localizedDescription)
        }
    }
}
