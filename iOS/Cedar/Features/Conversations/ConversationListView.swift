import SwiftUI

struct ConversationListView: View {
    @Environment(AppModel.self) private var app
    @State private var renaming: Conversation?
    @State private var newTitle = ""
    @State private var actionError: String?
    @State private var showingPrompt: Schedule?

    var body: some View {
        @Bindable var app = app
        List(selection: $app.selection) {
            switch app.sidebarMode {
            case .chats: chats
            case .starred: starredRows
            case .scheduled: ScheduleRows(actionError: $actionError, showingPrompt: $showingPrompt)
            }
        }
        .safeAreaInset(edge: .top) {
            Picker("Show", selection: $app.sidebarMode) {
                Text("Chats").tag(SidebarMode.chats)
                Text("Starred").tag(SidebarMode.starred)
                Text(app.scheduledUnread > 0 ? "Scheduled (\(app.scheduledUnread))" : "Scheduled")
                    .tag(SidebarMode.scheduled)
            }
            .pickerStyle(.segmented)
            .padding(.horizontal)
            .padding(.bottom, 8)
            .background(.bar)
        }
        .onChange(of: app.sidebarMode) { _, mode in
            switch mode {
            case .chats: break
            // Stars set on the web or the desktop app since the last refresh.
            case .starred: Task { await app.refresh() }
            case .scheduled: Task { await app.refreshSchedules() }
            }
        }
        .overlay {
            switch app.sidebarMode {
            case .chats:
                if app.conversations.isEmpty && app.listError == nil && !app.isRefreshing {
                    ContentUnavailableView("No Conversations", systemImage: "bubble.left.and.bubble.right",
                                           description: Text("Start a new chat to run the agent."))
                }
            case .starred:
                if app.starred.isEmpty && app.starredError == nil {
                    if app.starredLoaded {
                        ContentUnavailableView("No Starred Conversations", systemImage: "star",
                                               description: Text("Star a chat or a scheduled run, here or on the web, to keep it at hand on every device."))
                    } else {
                        ProgressView()
                    }
                }
            case .scheduled:
                if app.schedules.isEmpty && app.scheduleError == nil {
                    if app.schedulesLoaded {
                        ContentUnavailableView("No Scheduled Agents", systemImage: "clock",
                                               description: Text("On the web, the Schedule button turns a finished conversation into an agent that runs on its own."))
                    } else {
                        ProgressView()
                    }
                }
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
                    app.sidebarMode = .chats
                    app.newChat()
                } label: {
                    Label("New Chat", systemImage: "square.and.pencil")
                }
                .disabled(!app.settings.isConfigured)
            }
        }
        .sheet(item: $showingPrompt) { schedule in
            SchedulePromptView(schedule: schedule)
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

    @ViewBuilder
    private var chats: some View {
        if let error = app.listError {
            Section {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .font(.footnote)
            }
        }
        ForEach(app.conversations) { conversation in
            let starred = app.isStarred(conversationId: conversation.id)
            ConversationRow(title: app.title(for: conversation), conversation: conversation,
                            isActive: app.isActive(conversationId: conversation.id), starred: starred)
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
                .swipeActions(edge: .leading) {
                    StarButton(starred: starred) { setStarred(conversation, !starred) }
                }
                .contextMenu {
                    StarButton(starred: starred) { setStarred(conversation, !starred) }
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

    /// Chats and scheduled runs together, most recently starred first.
    @ViewBuilder
    private var starredRows: some View {
        if let error = app.starredError {
            Section {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .font(.footnote)
            }
        }
        ForEach(app.starred) { conversation in
            ConversationRow(title: app.title(for: conversation), conversation: conversation,
                            isActive: app.isActive(conversationId: conversation.id), starred: false)
                .tag(conversation.id)
                .swipeActions {
                    StarButton(starred: true) { setStarred(conversation, false) }
                }
                .contextMenu {
                    StarButton(starred: true) { setStarred(conversation, false) }
                    if let client = app.client {
                        Link(destination: client.webURL(conversationId: conversation.id)) {
                            Label("Open on the Web", systemImage: "safari")
                        }
                    }
                }
        }
    }

    private func setStarred(_ conversation: Conversation, _ on: Bool) {
        Task {
            do {
                try await app.setStarred(conversationId: conversation.id, on)
            } catch {
                actionError = error.localizedDescription
            }
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
    /// Marks the row with a star (the Chats list; the Starred list needs none).
    let starred: Bool

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
                    if starred { StarMark() }
                    if isActive {
                        ProgressView()
                            .controlSize(.mini)
                    }
                }
                HStack(spacing: 6) {
                    if conversation.scheduleId != nil {
                        Label(conversation.scheduleTitle ?? "Scheduled", systemImage: "clock")
                            .labelStyle(.titleAndIcon)
                            .lineLimit(1)
                        Text("·")
                    }
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
        .accessibilityLabel([title, starred ? "starred" : nil, conversation.unread ? "unread" : nil].compactMap { $0 }.joined(separator: ", "))
    }
}

/// Stars or unstars a conversation, in a swipe action or a context menu.
struct StarButton: View {
    let starred: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            starred
                ? Label("Unstar", systemImage: "star.slash")
                : Label("Star", systemImage: "star")
        }
        .tint(.yellow)
    }
}

/// The small star a starred conversation's row carries.
struct StarMark: View {
    var body: some View {
        Image(systemName: "star.fill")
            .font(.caption2)
            .foregroundStyle(.yellow)
            .accessibilityLabel("Starred")
    }
}
