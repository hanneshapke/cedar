import SwiftUI

/// Which agent's detail to show, in a sheet; `title` stands in until the agent is loaded.
struct AgentSelection: Identifiable, Hashable {
    let id: String
    var title = ""
}

/// The sidebar's Agents list: every agent under what starts it, with its trigger, where it
/// stands and its last run. Tapping one opens its detail; Run Now, Pause, Edit and Delete are a
/// swipe or a long press away. The Activity side lists the latest runs of all of them.
struct AgentRows: View {
    @Environment(AppModel.self) private var app
    @Binding var actionError: String?
    /// Opens an agent's detail.
    let show: (Agent) -> Void
    /// Opens the form for an agent.
    let edit: (Agent) -> Void
    /// Opens the form for a new agent.
    let create: () -> Void
    /// Asks, then deletes.
    let delete: (Agent) -> Void

    var body: some View {
        switch app.agentsTab {
        case .agents: agents
        case .activity: ActivityRows(actionError: $actionError)
        }
    }

    @ViewBuilder
    private var agents: some View {
        if let error = app.agentError {
            Section {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .font(.footnote)
                    .sidebarRow()
            }
        }
        if !app.agentsLoaded {
            if app.agentError == nil {
                ProgressView()
                    .frame(maxWidth: .infinity)
                    .listRowSeparator(.hidden)
                    .sidebarRow()
            }
        } else if app.agents.isEmpty {
            ContentUnavailableView {
                Label("No Agents Yet", systemImage: "clock.arrow.circlepath")
            } description: {
                Text("An agent is a prompt Cedar runs on its own: on a schedule, when something changes, when a webhook or your device calls it, or when mail, a calendar event or a new file comes up.")
            } actions: {
                Button {
                    create()
                } label: {
                    Label("New Agent", systemImage: "plus")
                }
                .buttonStyle(.borderedProminent)
            }
            .listRowSeparator(.hidden)
            .sidebarRow()
        } else {
            ForEach(Trigger.Group.allCases) { group in
                let members = app.agents.filter { $0.trigger.group == group }
                if !members.isEmpty {
                    Section {
                        ForEach(members) { agent in
                            row(agent)
                        }
                    } header: {
                        Text(group.title)
                    }
                }
            }
        }
    }

    private func row(_ agent: Agent) -> some View {
        Button {
            show(agent)
        } label: {
            AgentRow(agent: agent, unread: app.unreadAgents.contains(agent.id))
                .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .swipeActions {
            // Not destructive by role: it asks first, and the row stays until the answer.
            Button {
                delete(agent)
            } label: {
                Label("Delete", systemImage: "trash")
            }
            .tint(.red)
            Button {
                setEnabled(agent, !agent.enabled)
            } label: {
                agent.enabled
                    ? Label("Pause", systemImage: "pause")
                    : Label("Resume", systemImage: "play")
            }
            .tint(.orange)
            Button {
                runNow(agent)
            } label: {
                Label("Run Now", systemImage: "play.circle")
            }
            .tint(.accentColor)
        }
        .swipeActions(edge: .leading) {
            Button {
                edit(agent)
            } label: {
                Label("Edit", systemImage: "pencil")
            }
            .tint(.blue)
        }
        .contextMenu {
            Button {
                runNow(agent)
            } label: {
                Label("Run Now", systemImage: "play.circle")
            }
            .disabled(agent.lastRun?.isActive ?? false)
            Button {
                setEnabled(agent, !agent.enabled)
            } label: {
                agent.enabled
                    ? Label("Pause", systemImage: "pause")
                    : Label("Resume", systemImage: "play")
            }
            Button {
                edit(agent)
            } label: {
                Label("Edit", systemImage: "pencil")
            }
            Button(role: .destructive) {
                delete(agent)
            } label: {
                Label("Delete", systemImage: "trash")
            }
        }
        .sidebarRow()
    }

    private func runNow(_ agent: Agent) {
        Task {
            do {
                try await app.runNow(agent)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func setEnabled(_ agent: Agent, _ enabled: Bool) {
        Task {
            do {
                try await app.setEnabled(agent, enabled)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }
}

private struct AgentRow: View {
    let agent: Agent
    let unread: Bool

    var body: some View {
        let status = agent.status
        HStack(alignment: .center, spacing: 12) {
            RowIcon(systemImage: agent.enabled ? agent.trigger.systemImage : "pause",
                    tint: agent.enabled ? Theme.green : .secondary,
                    isActive: agent.lastRun?.isActive ?? false)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(agent.title)
                        .lineLimit(1)
                        .font(.body.weight(unread ? .semibold : .regular))
                        .foregroundStyle(Color.primary)
                    Spacer(minLength: 4)
                    if unread { UnreadDot() }
                }
                Group {
                    Text(agent.trigger.summary)
                        .lineLimit(1)
                    HStack(spacing: 4) {
                        AgentStatusLabel(status: status)
                        if let last = agent.lastRun {
                            if status != .idle { Text(verbatim: "·") }
                            RunOutcomeLabel(outcome: last.outcome)
                        }
                    }
                    .lineLimit(1)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            .opacity(agent.enabled ? 1 : 0.7)
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
        .accessibilityValue(unread ? L10n.string("Unread") : "")
    }
}

/// Where an agent stands, in a few words: paused, when it runs next, what it waits for.
struct AgentStatusLabel: View {
    let status: Agent.Status

    var body: some View {
        switch status {
        case .paused:
            Label("Paused", systemImage: "pause.circle")
                .labelStyle(.titleAndIcon)
        case .done:
            Label("Done", systemImage: "checkmark.circle")
                .labelStyle(.titleAndIcon)
        case .checkFailing:
            Label("Check failing", systemImage: "exclamationmark.triangle")
                .labelStyle(.titleAndIcon)
                .foregroundStyle(.red)
        case .nextRun(let date):
            Text("Next \(date, format: .relative(presentation: .named).locale(L10n.locale))")
        case .watching(let lastChecked):
            if let lastChecked {
                Text("Watching · checked \(lastChecked, format: .relative(presentation: .named).locale(L10n.locale))")
            } else {
                Text("Watching")
            }
        case .waitingForCalls:
            Text("Waiting for calls")
        case .callWaiting:
            Text("A call is waiting")
        case .waitingForDevice:
            Text("Waiting for your device")
        case .idle:
            EmptyView()
        }
    }
}

/// How a run went.
struct RunOutcomeLabel: View {
    let outcome: AgentRun.Outcome

    var body: some View {
        switch outcome {
        case .queued: Label("Queued", systemImage: "clock").labelStyle(.titleAndIcon)
        case .running: Label("Running", systemImage: "circle.dotted").labelStyle(.titleAndIcon)
        case .completed: Label("Done", systemImage: "checkmark.circle").labelStyle(.titleAndIcon)
        case .needsAnswer:
            Label("Needs answer", systemImage: "questionmark.bubble")
                .labelStyle(.titleAndIcon)
                .foregroundStyle(Theme.clay)
        case .failed:
            Label("Failed", systemImage: "xmark.octagon")
                .labelStyle(.titleAndIcon)
                .foregroundStyle(.red)
        case .cancelled: Label("Stopped", systemImage: "stop.circle").labelStyle(.titleAndIcon)
        case .notStarted:
            Label("Didn't start", systemImage: "exclamationmark.triangle")
                .labelStyle(.titleAndIcon)
                .foregroundStyle(.red)
        }
    }
}

/// What started a run: the server's one line for it, or a chip for one started with Run Now.
/// Nothing for a run from before causes were kept.
struct RunCauseLabel: View {
    let run: AgentRun

    var body: some View {
        if let summary = run.startedBy {
            Text(verbatim: L10n.labeled(L10n.string("Started by"), summary))
        } else if run.manual {
            Text("Run now")
                .font(.caption2.weight(.medium))
                .padding(.horizontal, 7)
                .padding(.vertical, 2)
                .background(Theme.raised, in: Capsule())
        }
    }
}

/// The Activity side of the Agents list: the latest runs of every agent, newest first, each
/// with its agent, what started it and how it went. Tapping one opens its conversation.
struct ActivityRows: View {
    @Environment(AppModel.self) private var app
    @Binding var actionError: String?

    var body: some View {
        if let error = app.activityError {
            Section {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .font(.footnote)
                    .sidebarRow()
            }
        }
        if !app.activityLoaded {
            if app.activityError == nil {
                ProgressView()
                    .frame(maxWidth: .infinity)
                    .listRowSeparator(.hidden)
                    .sidebarRow()
            }
        } else if app.activity.isEmpty {
            ContentUnavailableView("No Activity Yet", systemImage: "clock.arrow.circlepath",
                                   description: Text("Every run your agents start shows up here, with what started it."))
                .listRowSeparator(.hidden)
                .sidebarRow()
        } else {
            ForEach(app.activity) { item in
                if let conversationId = item.run.conversationId {
                    let starred = app.isStarred(conversationId: conversationId)
                    ActivityRow(item: item, starred: starred)
                        .tag(conversationId)
                        .sidebarRow(selected: app.selection == conversationId)
                        .swipeActions {
                            StarButton(starred: starred) { setStarred(conversationId, !starred) }
                        }
                        .contextMenu {
                            StarButton(starred: starred) { setStarred(conversationId, !starred) }
                        }
                } else {
                    // It never started: nothing to open (or star).
                    ActivityRow(item: item, starred: false)
                        .sidebarRow()
                }
            }
            if app.activityNext != nil {
                Button {
                    Task { await app.loadMoreActivity() }
                } label: {
                    HStack {
                        Text("Load More")
                        Spacer()
                        if app.isLoadingActivity { ProgressView() }
                    }
                }
                .disabled(app.isLoadingActivity)
                .sidebarRow()
            }
        }
    }

    private func setStarred(_ conversationId: String, _ on: Bool) {
        Task {
            do {
                try await app.setStarred(conversationId: conversationId, on)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }
}

private struct ActivityRow: View {
    let item: ActivityItem
    let starred: Bool

    var body: some View {
        let run = item.run
        HStack(alignment: .center, spacing: 12) {
            RowIcon(systemImage: item.agent?.triggerKind.map { Trigger.systemImage(forKind: $0) } ?? "clock.arrow.circlepath",
                    tint: run.outcome == .needsAnswer ? Theme.clay : Theme.green,
                    isActive: run.isActive)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(title)
                        .lineLimit(1)
                        .font(.body.weight(run.unread ? .semibold : .regular))
                    if starred { StarMark() }
                    Spacer(minLength: 4)
                    Text(run.startedAt, format: .relative(presentation: .named, unitsStyle: .abbreviated).locale(L10n.locale))
                        .font(.caption.weight(run.unread ? .semibold : .regular))
                        .foregroundStyle(run.unread ? Theme.green : Color.secondary)
                        .lineLimit(1)
                        .layoutPriority(1)
                }
                Group {
                    RunCauseLabel(run: run)
                        .lineLimit(2)
                    HStack(spacing: 6) {
                        RunOutcomeLabel(outcome: run.outcome)
                        Spacer(minLength: 4)
                        if run.unread { UnreadDot() }
                    }
                    if run.outcome == .notStarted, let error = run.error {
                        Text(error)
                            .font(.caption2)
                            .lineLimit(2)
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }

    private var title: String {
        let title = (item.agent?.title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return title.isEmpty ? L10n.string("Agent") : title
    }
}

/// One run in an agent's history: when it was due, what started it and how it went.
struct RunRow: View {
    let run: AgentRun
    let starred: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(Theme.green)
                .frame(width: 7, height: 7)
                .padding(.top, 7)
                .opacity(run.unread ? 1 : 0)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 4) {
                    Text(run.dueAt, format: .dateTime.weekday(.abbreviated).month(.abbreviated).day().hour().minute().locale(L10n.locale))
                        .font(run.unread ? .subheadline.weight(.semibold) : .subheadline)
                    if starred { StarMark() }
                }
                Group {
                    RunCauseLabel(run: run)
                    RunOutcomeLabel(outcome: run.outcome)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if run.outcome == .notStarted, let error = run.error {
                    Text(error)
                        .font(.caption2)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
        }
        .padding(.vertical, 1)
        .accessibilityElement(children: .combine)
    }
}

/// The badge a conversation that is an agent's run carries above its messages: the agent's
/// title, and a tap to its detail.
struct AgentBadge: View {
    let agent: AgentRef
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 6) {
                Image(systemName: agent.triggerKind.map { Trigger.systemImage(forKind: $0) } ?? "clock.arrow.circlepath")
                Text(title)
                    .lineLimit(1)
                Image(systemName: "chevron.right")
                    .font(.caption2.weight(.semibold))
            }
            .font(.footnote.weight(.medium))
            .foregroundStyle(Theme.green)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(Theme.green.opacity(0.12), in: Capsule())
            .contentShape(Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(Text(verbatim: L10n.labeled(L10n.string("Agent"), title)))
        .accessibilityHint("Opens the agent")
        .frame(maxWidth: .infinity)
        .padding(.vertical, 6)
        .background(Theme.canvas)
    }

    private var title: String {
        let title = agent.title.trimmingCharacters(in: .whitespacesAndNewlines)
        return title.isEmpty ? L10n.string("Agent") : title
    }
}
