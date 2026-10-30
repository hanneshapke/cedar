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
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Theme.canvas)
        .safeAreaInset(edge: .top, spacing: 0) {
            SidebarSwitch(mode: $app.sidebarMode, scheduledUnread: app.scheduledUnread)
                .padding(.horizontal)
                .padding(.top, 4)
                .padding(.bottom, 10)
                .background(Theme.canvas)
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
        .navigationTitle(Text(verbatim: "cedar"))
        // The wordmark stands in for the title, so it shows whatever the list does under it.
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(Theme.canvas, for: .navigationBar)
        .refreshable { await app.refresh() }
        .toolbar {
            ToolbarItem(placement: .principal) {
                CedarWordmark()
            }
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
                    .sidebarRow()
            }
        }
        ForEach(app.conversations) { conversation in
            let starred = app.isStarred(conversationId: conversation.id)
            ConversationRow(title: app.title(for: conversation), conversation: conversation,
                            isActive: app.isActive(conversationId: conversation.id), starred: starred)
                .tag(conversation.id)
                .sidebarRow(selected: app.selection == conversation.id)
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
                    .sidebarRow()
            }
        }
        ForEach(app.starred) { conversation in
            ConversationRow(title: app.title(for: conversation), conversation: conversation,
                            isActive: app.isActive(conversationId: conversation.id), starred: false)
                .tag(conversation.id)
                .sidebarRow(selected: app.selection == conversation.id)
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
        HStack(alignment: .center, spacing: 12) {
            RowIcon(systemImage: conversation.scheduleId != nil ? "clock.arrow.circlepath" : "bubble.left.and.text.bubble.right",
                    tint: conversation.needsAnswer ? Theme.clay : Theme.green, isActive: isActive)
            VStack(alignment: .leading, spacing: 4) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(title)
                        .lineLimit(1)
                        .font(.body.weight(conversation.unread ? .semibold : .regular))
                    if starred { StarMark() }
                    Spacer(minLength: 4)
                    Text(conversation.updatedAt, format: .relative(presentation: .named, unitsStyle: .abbreviated).locale(L10n.locale))
                        .font(.caption.weight(conversation.unread ? .semibold : .regular))
                        .foregroundStyle(conversation.unread ? Theme.green : Color.secondary)
                        .lineLimit(1)
                        .layoutPriority(1)
                }
                HStack(spacing: 6) {
                    if conversation.scheduleId != nil {
                        Label(conversation.scheduleTitle ?? L10n.string("Scheduled"), systemImage: "clock")
                            .labelStyle(.titleAndIcon)
                            .lineLimit(1)
                    }
                    if let model = conversation.model, !model.isEmpty {
                        if conversation.scheduleId != nil { Text(verbatim: "·") }
                        Text(verbatim: model)
                            .lineLimit(1)
                    }
                    Spacer(minLength: 4)
                    if conversation.needsAnswer {
                        Text("Needs answer")
                            .font(.caption2.weight(.semibold))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(Theme.clay.opacity(0.15), in: Capsule())
                            .foregroundStyle(Theme.clay)
                    } else if conversation.unread {
                        UnreadDot()
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
        .accessibilityLabel([title, starred ? L10n.string("starred") : nil, conversation.unread ? L10n.string("unread") : nil]
            .compactMap { $0 }.joined(separator: ", "))
    }
}

/// A row's leading tile: what the row is, or a spinner while its agent runs.
struct RowIcon: View {
    let systemImage: String
    var tint: Color = Theme.green
    var isActive = false

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(tint.opacity(0.13))
            if isActive {
                ProgressView()
                    .controlSize(.small)
                    .tint(tint)
            } else {
                Image(systemName: systemImage)
                    .font(.system(size: 16, weight: .medium))
                    .foregroundStyle(tint)
            }
        }
        .frame(width: 42, height: 42)
        .accessibilityHidden(true)
    }
}

/// The dot an unread row carries.
struct UnreadDot: View {
    var body: some View {
        Circle()
            .fill(Theme.green)
            .frame(width: 9, height: 9)
            .accessibilityHidden(true)
    }
}

/// The sidebar's Chats | Starred | Scheduled switch.
private struct SidebarSwitch: View {
    @Binding var mode: SidebarMode
    let scheduledUnread: Int
    @Namespace private var selection

    var body: some View {
        HStack(spacing: 2) {
            segment(.chats) { Text("Chats") }
            segment(.starred) { Text("Starred") }
            segment(.scheduled) {
                HStack(spacing: 5) {
                    Text("Scheduled")
                    if scheduledUnread > 0 {
                        Text(verbatim: "\(scheduledUnread)")
                            .font(.caption2.weight(.bold))
                            .monospacedDigit()
                            .foregroundStyle(.white)
                            .padding(.horizontal, 5)
                            .frame(minWidth: 17, minHeight: 17)
                            .background(Theme.clay, in: Capsule())
                    }
                }
            }
        }
        .padding(3)
        .background(Theme.raised, in: Capsule())
        .accessibilityElement(children: .contain)
    }

    private func segment<Content: View>(_ value: SidebarMode, @ViewBuilder label: () -> Content) -> some View {
        let selected = mode == value
        return Button {
            withAnimation(.snappy(duration: 0.25)) { mode = value }
        } label: {
            label()
                .font(.subheadline.weight(selected ? .semibold : .medium))
                .foregroundStyle(selected ? Color.primary : Color.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.8)
                .frame(maxWidth: .infinity)
                .padding(.vertical, 8)
                .background {
                    if selected {
                        Capsule()
                            .fill(Theme.surface)
                            .shadow(color: .black.opacity(0.1), radius: 4, y: 1)
                            .matchedGeometryEffect(id: "selection", in: selection)
                    }
                }
                .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(selected ? [.isSelected] : [])
    }
}

extension View {
    /// A sidebar row on the canvas, with a soft green tile behind the open one.
    func sidebarRow(selected: Bool = false) -> some View {
        listRowBackground(
            RoundedRectangle(cornerRadius: 14, style: .continuous)
                .fill(selected ? Theme.green.opacity(0.12) : Color.clear)
                .padding(.horizontal, 8)
                .background(Theme.canvas)
        )
        .listRowSeparatorTint(Theme.line)
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
