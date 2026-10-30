import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.scenePhase) private var scenePhase
    private let language = LanguageStore.shared

    var body: some View {
        Group {
            if language.hasChosen {
                main
            } else {
                // Before anything else, the server's settings included.
                LanguageOnboardingView { tag in app.chooseLanguage(tag) }
            }
        }
        // Text looks itself up, and formats, in the picked language; a change shows at once.
        .environment(\.locale, language.locale)
    }

    private var main: some View {
        @Bindable var app = app
        return NavigationSplitView(preferredCompactColumn: $app.compactColumn) {
            ConversationListView()
        } detail: {
            if let selection = app.selection {
                let chat = app.chat(for: selection)
                // Keyed by the chat, not the selection: a draft keeps its view when it becomes a
                // conversation.
                ChatView(chat: chat)
                    .id(chat.key)
            } else {
                VStack(spacing: 18) {
                    CedarBadge(size: 88)
                    CedarWordmark(size: 28)
                    Text("Ask the agent to search, write and run code, and build things in its sandbox.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                        .frame(maxWidth: 360)
                    Button {
                        app.newChat()
                    } label: {
                        Label("New Chat", systemImage: "square.and.pencil")
                    }
                    .buttonStyle(ProminentButtonStyle())
                    .frame(maxWidth: 240)
                    .padding(.top, 6)
                }
                .padding()
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Theme.canvas)
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
