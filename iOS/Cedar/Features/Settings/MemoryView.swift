import SwiftUI

/// Settings → Memory: what Cedar remembers about the user across conversations, newest first, as
/// the web UI's Settings → Memory lists it. Swipe to delete one; Delete All forgets everything
/// after asking. Deletions apply at once, like the language, not on Save: the next message, in
/// any conversation, goes without them.
struct MemoryView: View {
    @Environment(AppModel.self) private var app
    @State private var confirmingDeleteAll = false
    @State private var actionError: String?

    var body: some View {
        List {
            if let error = app.memoryError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .font(.footnote)
                }
            }
            if !app.memoryLoaded {
                if app.memoryError == nil {
                    ProgressView().frame(maxWidth: .infinity)
                }
            } else if !app.memorySupported {
                ContentUnavailableView("Memory", systemImage: "bookmark",
                                       description: Text("This server doesn't remember anything between conversations yet. Update cedar to use memory."))
            } else if app.memoryItems.isEmpty {
                ContentUnavailableView("Nothing Remembered Yet", systemImage: "bookmark",
                                       description: Text("When you tell Cedar something lasting, like your name, a preference or a project, or ask it to remember something, it appears here."))
            } else {
                Section {
                    // Newest first here; the agent reads them oldest first.
                    ForEach(app.memoryItems.reversed()) { item in
                        row(item)
                            .swipeActions {
                                Button(role: .destructive) {
                                    delete(item)
                                } label: {
                                    Label("Delete", systemImage: "trash")
                                }
                            }
                    }
                } header: {
                    Text(countText)
                } footer: {
                    Text("This is exactly what Cedar sees. Memory stays on your own server and is never shared with a third party. Settings → Tools in the web UI turns it off.")
                }
            }
        }
        .navigationTitle("Memory")
        .toolbar {
            if !app.memoryItems.isEmpty {
                ToolbarItem(placement: .primaryAction) {
                    Button("Delete All", role: .destructive) { confirmingDeleteAll = true }
                }
            }
        }
        .task { await app.refreshMemory() }
        .refreshable { await app.refreshMemory() }
        .alert("Forget Everything?", isPresented: $confirmingDeleteAll) {
            Button("Delete All", role: .destructive) { deleteAll() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(deleteAllMessage)
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

    /// One memory: its text, when it was saved, and the conversation it came from (a tap opens it).
    @ViewBuilder
    private func row(_ item: MemoryItem) -> some View {
        let content = VStack(alignment: .leading, spacing: 3) {
            Text(item.text)
            Text(verbatim: "\(item.createdAt.formatted(.relative(presentation: .named).locale(L10n.locale))) · \(source(item))")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        if let conversationId = item.conversationId {
            Button {
                app.showingSettings = false
                app.selection = conversationId
                app.compactColumn = .detail
            } label: {
                content
            }
            .foregroundStyle(.primary)
        } else {
            content
        }
    }

    private func source(_ item: MemoryItem) -> String {
        guard item.conversationId != nil else { return L10n.string("Not linked to a conversation") }
        let title = item.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return title.isEmpty ? L10n.string("New chat") : title
    }

    private var countText: String {
        let count = app.memoryItems.count
        if let max = app.memoryLimits?.maxItems, max > 0 {
            return L10n.format("%lld of %lld remembered", count, max)
        }
        return L10n.format(count == 1 ? "%lld memory" : "%lld memories", count)
    }

    private var deleteAllMessage: String {
        let count = app.memoryItems.count
        return L10n.format(count == 1
                           ? "Cedar will forget %lld memory it has about you. This can't be undone."
                           : "Cedar will forget all %lld memories it has about you. This can't be undone.", count)
    }

    private func delete(_ item: MemoryItem) {
        Task {
            do {
                try await app.deleteMemory(item)
            } catch {
                actionError = error.localizedDescription
                await app.refreshMemory()
            }
        }
    }

    private func deleteAll() {
        Task {
            do {
                try await app.deleteAllMemory()
            } catch {
                actionError = error.localizedDescription
                await app.refreshMemory()
            }
        }
    }
}
