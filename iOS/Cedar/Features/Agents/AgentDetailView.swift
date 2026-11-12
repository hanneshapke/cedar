import SwiftUI
import UIKit

/// One agent, in a sheet: what starts it and where it stands, Run Now and Pause, its prompt and
/// settings, and its runs, newest first, each with what started it. A webhook agent also shows
/// its address, the secret (once, right after it was made or replaced) and how to call it.
/// Edit opens the form; Delete asks first.
struct AgentDetailView: View {
    let agentId: String
    /// Shown until the agent is loaded.
    var title = ""
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var editing: AgentFormContext?
    @State private var confirmingDelete = false
    @State private var confirmingRotate = false
    @State private var actionError: String?
    @State private var busy = false
    @State private var loaded = false

    private var agent: Agent? { app.agents.first { $0.id == agentId } }

    var body: some View {
        NavigationStack {
            List {
                if let agent {
                    content(agent)
                } else if loaded {
                    ContentUnavailableView("This Agent No Longer Exists", systemImage: "questionmark.circle",
                                           description: Text("It may have been deleted on another device."))
                } else {
                    ProgressView().frame(maxWidth: .infinity)
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.canvas)
            .navigationTitle(agent?.title ?? title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
                ToolbarItem(placement: .primaryAction) {
                    Button("Edit") {
                        if let agent { editing = AgentFormContext(agent: agent, limits: app.agentLimits) }
                    }
                    .disabled(agent == nil)
                }
            }
            .task {
                await app.loadAgent(id: agentId)
                loaded = true
            }
            .refreshable { await app.loadAgent(id: agentId) }
            .sheet(item: $editing) { context in
                AgentFormView(context: context)
            }
            .alert("Delete This Agent?", isPresented: $confirmingDelete) {
                Button("Delete", role: .destructive) { delete() }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("The agent and the conversations of all its runs will be deleted. This can't be undone.")
            }
            .alert("Make a New Secret?", isPresented: $confirmingRotate) {
                Button("New Secret", role: .destructive) { rotate() }
                Button("Cancel", role: .cancel) {}
            } message: {
                Text("The current secret stops working at once. Whatever calls this webhook needs the new one.")
            }
            .alert("Could Not Do That", isPresented: Binding(
                get: { actionError != nil },
                set: { if !$0 { actionError = nil } }
            )) {
                Button("OK", role: .cancel) {}
            } message: {
                Text(actionError ?? "")
            }
            .onDisappear {
                // The secret was on screen; it isn't kept. (The form over this sheet isn't leaving.)
                if editing == nil { app.forgetSecret(agentId: agentId) }
            }
        }
    }

    // MARK: Sections

    @ViewBuilder
    private func content(_ agent: Agent) -> some View {
        Section {
            Label {
                VStack(alignment: .leading, spacing: 3) {
                    Text(agent.trigger.summary)
                    HStack(spacing: 4) {
                        AgentStatusLabel(status: agent.status)
                    }
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                }
            } icon: {
                Image(systemName: agent.enabled ? agent.trigger.systemImage : "pause")
                    .foregroundStyle(agent.enabled ? Theme.green : Color.secondary)
            }
            if case .checkFailing(let error) = agent.status {
                Text(error)
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }
            if case .device = agent.trigger {
                Text("Start it from the Shortcuts app with the Run Cedar Agent action, by hand or from an automation.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            if !agent.trigger.isKnown {
                Text("This trigger was made by a newer version of Cedar. It runs as usual, but this version of the app can only show it.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        } header: {
            Text(agent.trigger.group.title)
        }

        Section {
            Button {
                runNow(agent)
            } label: {
                HStack {
                    Label("Run Now", systemImage: "play.circle")
                    Spacer()
                    if busy { ProgressView() }
                }
            }
            .disabled(busy || (agent.lastRun?.isActive ?? false))
            Button {
                setEnabled(agent, !agent.enabled)
            } label: {
                agent.enabled
                    ? Label("Pause", systemImage: "pause")
                    : Label("Resume", systemImage: "play")
            }
            .disabled(busy)
        }

        if let hook = agent.hook {
            webhook(agent, hook)
        }

        Section {
            Text(agent.prompt)
                .textSelection(.enabled)
        } header: {
            Text("Prompt")
        }

        Section {
            LabeledContent("AI Model") {
                Text(verbatim: modelText(agent))
            }
            LabeledContent("Notifications") {
                Text(verbatim: agent.notify.map { NotificationStatus.label($0) } ?? L10n.string("Default"))
            }
            LabeledContent("Tools") {
                Text(verbatim: toolsText(agent))
            }
            if agent.trigger.byEvent {
                LabeledContent("Limits") {
                    Text(verbatim: limitsText(agent))
                }
            }
        } header: {
            Text("Settings")
        }

        Section {
            if let runs = app.agentRuns[agent.id] {
                if runs.isEmpty {
                    Text("No runs yet")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                ForEach(runs) { run in
                    if let conversationId = run.conversationId {
                        Button {
                            open(conversationId)
                        } label: {
                            RunRow(run: run, starred: app.isStarred(conversationId: conversationId))
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    } else {
                        // It never started: nothing to open.
                        RunRow(run: run, starred: false)
                    }
                }
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        } header: {
            Text("Runs")
        }

        Section {
            Button("Delete Agent", role: .destructive) { confirmingDelete = true }
                .disabled(busy)
        }
    }

    /// A webhook agent's address and secret, and how to call it.
    @ViewBuilder
    private func webhook(_ agent: Agent, _ hook: Agent.Hook) -> some View {
        let secret = app.agentSecrets[agent.id]
        Section {
            if let url = hook.url {
                CopyRow(title: "Address", value: url)
            }
            if let secret {
                CopyRow(title: "Secret", value: secret)
                Label("Copy the secret now. It is shown only this once.", systemImage: "exclamationmark.circle")
                    .font(.footnote)
                    .foregroundStyle(Theme.clay)
            } else if let last4 = hook.secretLast4, !last4.isEmpty {
                LabeledContent("Secret") {
                    Text(verbatim: "••••\(last4)").monospaced()
                }
            }
            if let url = hook.url {
                CopyRow(title: "Example", value: Self.curl(url: url, secret: secret))
            }
            Button("New Secret") { confirmingRotate = true }
                .disabled(busy)
        } header: {
            Text("Webhook")
        } footer: {
            Text("Send a POST request to the address with the secret as a bearer token. What it sends in the body is handed to the run as data. The sender has to be able to reach your server: an address on a private network or a tailnet only works from inside it.")
        }
    }

    /// The call that starts a run, with the secret when it is known.
    static func curl(url: String, secret: String?) -> String {
        "curl -X POST -H \"Authorization: Bearer \(secret ?? "<secret>")\" -H \"content-type: application/json\" -d '{\"door\":\"open\"}' \(url)"
    }

    // MARK: Words

    private func modelText(_ agent: Agent) -> String {
        guard let id = agent.model, !id.isEmpty else { return L10n.string("Server default") }
        let label = app.models.first { $0.id == id }?.label ?? id
        guard let reasoning = agent.reasoning, !reasoning.isEmpty else { return label }
        return "\(label) · \(reasoning.capitalized)"
    }

    private func toolsText(_ agent: Agent) -> String {
        guard let tools = agent.tools else { return L10n.string("Server defaults") }
        if tools.contains("*") { return L10n.string("Everything") }
        return L10n.format(tools.count == 1 ? "%lld tool or group" : "%lld tools and groups", tools.count)
    }

    private func limitsText(_ agent: Agent) -> String {
        let seconds = agent.minIntervalSecs ?? app.agentLimits.minIntervalSecs
        let runs = agent.maxRunsPerDay ?? app.agentLimits.maxRunsPerDay
        return L10n.format("Every %@ at most, %lld a day", Trigger.duration(max(1, seconds / 60)), runs)
    }

    // MARK: Actions

    /// Opens a run's conversation in the detail column, under this sheet.
    private func open(_ conversationId: String) {
        app.selection = conversationId
        app.compactColumn = .detail
        dismiss()
    }

    private func runNow(_ agent: Agent) {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.runNow(agent)
                // The run's conversation is open under this sheet.
                dismiss()
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func setEnabled(_ agent: Agent, _ enabled: Bool) {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.setEnabled(agent, enabled)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func rotate() {
        guard let agent else { return }
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.rotateHook(agent)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func delete() {
        guard let agent else { return }
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.deleteAgent(agent)
                dismiss()
            } catch {
                // 409 while one of its runs is still going: the server says so.
                actionError = error.localizedDescription
                await app.loadAgent(id: agentId)
            }
        }
    }
}

/// A value to copy (an address, a secret, a command), under its name.
private struct CopyRow: View {
    let title: LocalizedStringKey
    let value: String
    @State private var copied = false

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            VStack(alignment: .leading, spacing: 3) {
                Text(title)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                Text(verbatim: value)
                    .font(.footnote.monospaced())
                    .textSelection(.enabled)
            }
            Spacer(minLength: 4)
            Button {
                UIPasteboard.general.string = value
                copied = true
            } label: {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.footnote)
            }
            .buttonStyle(.borderless)
            .accessibilityLabel("Copy")
        }
        .onChange(of: value) { copied = false }
    }
}
