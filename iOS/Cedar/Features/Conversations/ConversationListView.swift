import SwiftUI

struct ConversationListView: View {
    @Environment(AppModel.self) private var app
    @State private var renaming: Conversation?
    @State private var newTitle = ""
    @State private var deleteError: String?

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
                ConversationRow(title: app.title(for: conversation), conversation: conversation)
                    .tag(conversation.id)
                    .swipeActions {
                        Button(role: .destructive) {
                            delete(conversation)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                        .disabled(conversation.isActive)
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
                        Button(role: .destructive) {
                            delete(conversation)
                        } label: {
                            Label("Delete", systemImage: "trash")
                        }
                        .disabled(conversation.isActive)
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
                if let renaming { app.rename(renaming, to: newTitle) }
                renaming = nil
            }
            Button("Cancel", role: .cancel) { renaming = nil }
        }
        .alert("Could Not Delete", isPresented: Binding(
            get: { deleteError != nil },
            set: { if !$0 { deleteError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(deleteError ?? "")
        }
    }

    private func delete(_ conversation: Conversation) {
        Task {
            do {
                try await app.delete(conversation)
            } catch {
                deleteError = error.localizedDescription
                await app.refresh()
            }
        }
    }
}

private struct ConversationRow: View {
    let title: String
    let conversation: Conversation

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(title)
                    .lineLimit(1)
                    .font(.body)
                if conversation.isActive {
                    ProgressView()
                        .controlSize(.mini)
                }
            }
            HStack(spacing: 4) {
                Text(conversation.updatedAt, format: .relative(presentation: .named))
                if conversation.runs.count > 1 {
                    Text("· \(conversation.runs.count) messages")
                }
                if conversation.latest.status == .failed {
                    Image(systemName: "exclamationmark.circle")
                        .foregroundStyle(.red)
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
        .padding(.vertical, 2)
    }
}
