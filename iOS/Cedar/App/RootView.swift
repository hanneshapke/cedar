import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        @Bindable var app = app
        NavigationSplitView(preferredCompactColumn: $app.compactColumn) {
            ConversationListView()
        } detail: {
            if let selection = app.selection {
                let chat = app.chat(for: selection)
                // Keyed by the chat, not the selection: a draft keeps its view when it becomes a
                // conversation.
                ChatView(chat: chat)
                    .id(chat.key)
            } else {
                ContentUnavailableView {
                    Label("cedar", systemImage: "leaf")
                } description: {
                    Text("Ask the agent to write and run code in its sandbox.")
                } actions: {
                    Button("New Chat") { app.newChat() }
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .sheet(isPresented: $app.showingSettings) {
            SettingsView()
        }
        .task { await app.refresh() }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { Task { await app.refresh() } }
        }
    }
}
