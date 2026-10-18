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
                    Text("Ask the agent to search, write and run code, and build things in its sandbox.")
                } actions: {
                    Button("New Chat") { app.newChat() }
                        .buttonStyle(.borderedProminent)
                }
            }
        }
        .sheet(isPresented: $app.showingSettings) {
            SettingsView()
        }
        .onChange(of: app.selection) { _, id in
            if let id, app.conversation(id: id) != nil { app.markSeen(conversationId: id) }
        }
        .onChange(of: scenePhase) { _, phase in
            switch phase {
            case .active: app.becameActive()
            case .background, .inactive: app.isActive = false
            @unknown default: break
            }
        }
    }
}
