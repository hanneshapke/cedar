import SwiftUI

struct ConversationListView: View {
    @Environment(AppModel.self) private var app
    @State private var renaming: Conversation?
    @State private var newTitle = ""
    @State private var actionError: String?

    var body: some View {
        @Bindable var app = app
        List(selection: $app.selection) {
            if let error = app.listError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .font(.footnote)
                }
            }
            ForEach(app.conversations) { conversation in
                ConversationRow(title: app.title(for: conversation), conversation: conversation, isActive: app.isActive(conversationId: conversation.id))
                    .tag(conversation.id)
                    .swipeActions {
                        Button(role: .destructive) {
                            delete(conversation)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                        Button {
                            newTitle = app.title(for: conversation)
                            renaming = conversation
                        } label: {
                            Label("Rename", systemImage: "pencil")
                        }
                        .tint(.orange)
                    }
                    .contextMenu {
                        Button {
                            newTitle = app.title(for: conversation)
                            renaming = conversation
                        } label: {
                            Label("Rename", systemImage: "pencil")
                        }
                        if let client = app.client {
                            Link(destination: client.webURL(conversationId: conversation.id)) {
                                Label("Open on the Web", systemImage: "safari")
                            }
                        }
                        Button(role: .destructive) {
                            delete(conversation)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                    }
            }
        }
        .overlay {
            if app.conversations.isEmpty && app.listError == nil && !app.isRefreshing {
                ContentUnavailableView("No Conversations", systemImage: "bubble.left.and.bubble.right",
                                       description: Text("Start a new chat to run the agent."))
            }
        }
        .navigationTitle("cedar")
        .refreshable { await app.refresh() }
        .toolbar {
            ToolbarItem(placement: .topBarLeading) {
                Button {
                    app.showingSettings = true
                } label: {
                    Label("Settings", systemImage: "gearshape")
                }
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button {
                    app.newChat()
                } label: {
                    Label("New Chat", systemImage: "square.and.pencil")
                }
                .disabled(!app.settings.isConfigured)
            }
        }
        .alert("Rename Conversation", isPresented: Binding(
            get: { renaming != nil },
            set: { if !$0 { renaming = nil } }
        )) {
            TextField("Title", text: $newTitle)
            Button("Save") {
                if let renaming { rename(renaming, to: newTitle) }
                renaming = nil
            }
            Button("Cancel", role: .cancel) { renaming = nil }
        }
        .alert("Could Not Do That", isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
    }

    private func rename(_ conversation: Conversation, to title: String) {
        let title = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !title.isEmpty else { return }
        Task {
            do {
                try await app.rename(conversation, to: title)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func delete(_ conversation: Conversation) {
        Task {
            do {
                try await app.delete(conversation)
            } catch {
                actionError = error.localizedDescription
                await app.refresh()
            }
        }
    }
}

private struct ConversationRow: View {
    let title: String
    let conversation: Conversation
    let isActive: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(Color.accentColor)
                .frame(width: 8, height: 8)
                .padding(.top, 7)
                .opacity(conversation.unread ? 1 : 0)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(title)
                        .lineLimit(1)
                        .font(conversation.unread ? .body.weight(.semibold) : .body)
                    if isActive {
                        ProgressView()
                            .controlSize(.mini)
                    }
                }
                HStack(spacing: 6) {
                    Text(conversation.updatedAt, format: .relative(presentation: .named))
                    if conversation.needsAnswer {
                        Text("Needs answer")
                            .font(.caption2.weight(.semibold))
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(Color.orange.opacity(0.18), in: Capsule())
                            .foregroundStyle(.orange)
                    } else if conversation.unread {
                        Text("Unread")
                    }
                    if let model = conversation.model, !model.isEmpty {
                        Text("· \(model)")
                            .lineLimit(1)
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 2)
        .accessibilityLabel(conversation.unread ? "\(title), unread" : title)
    }
}
