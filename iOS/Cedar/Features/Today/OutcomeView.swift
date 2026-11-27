import SwiftUI

/// What one ask came to, in a sheet: the answer rather than the transcript. What was asked, the
/// gist as the headline, the answer, then what the turn left behind (the files it wrote or
/// changed, what it remembered, the record rows it saved, the follow-up it suggested, what it
/// held for your OK), with its steps and sources folded underneath. Keep going opens the
/// conversation, where the next message is sent.
struct OutcomeView: View {
    let turnId: String
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var outcome: AskOutcome?
    @State private var error: String?
    /// What was decided about its follow-ups here, by the suggesting call's id.
    @State private var decisions: [String: FollowUpDecision] = [:]
    /// The memories forgotten from here.
    @State private var forgotten: Set<String> = []
    @State private var showingAgent: AgentSelection?
    @State private var showingHow = false
    @State private var busy = false
    @State private var actionError: String?

    var body: some View {
        NavigationStack {
            Group {
                if let outcome {
                    ScrollView {
                        content(outcome)
                            .padding()
                            .frame(maxWidth: 720, alignment: .leading)
                            .frame(maxWidth: .infinity)
                    }
                } else if let error {
                    ContentUnavailableView {
                        Label("Couldn't Load This", systemImage: "exclamationmark.triangle")
                    } description: {
                        Text(error)
                    } actions: {
                        Button("Try Again") { Task { await load() } }
                    }
                } else {
                    ProgressView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.canvas)
            .navigationTitle("Today")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Close") { dismiss() }
                }
            }
            .task(id: turnId) {
                await load()
                // A response still going is followed until it ends.
                while outcome?.state == .working, !Task.isCancelled {
                    try? await Task.sleep(for: .seconds(3))
                    await load()
                }
            }
            .sheet(item: $showingAgent) { selection in
                AgentDetailView(agentId: selection.id, title: selection.title)
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
    }

    // MARK: Parts

    @ViewBuilder
    private func content(_ outcome: AskOutcome) -> some View {
        VStack(alignment: .leading, spacing: 22) {
            head(outcome)
            if outcome.state == .working {
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Working on it")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                    Spacer(minLength: 4)
                    Button("Stop") { stop() }
                        .buttonStyle(.bordered)
                        .disabled(busy)
                }
            }
            if let answer = outcome.turn.finalText, !answer.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                MarkdownView(text: answer)
            }
            if !outcome.held.isEmpty {
                part(L10n.string(outcome.held.contains(where: \.isOpen) ? "Waiting for your OK" : "From the outbox")) {
                    ForEach(outcome.heldOpenFirst) { item in
                        HeldCallCard(id: item.id)
                    }
                }
            }
            if !outcome.files.isEmpty, let client = app.client {
                part(L10n.string("Files")) {
                    DocumentsView(paths: outcome.files.map(\.path), conversationId: outcome.conversation.id, client: client)
                }
            }
            if !outcome.memories.isEmpty {
                part(L10n.string("Remembered")) {
                    ForEach(outcome.memories) { memory in
                        memoryRow(memory)
                    }
                }
            }
            if !outcome.records.isEmpty {
                part(L10n.string("Saved to your records")) {
                    ForEach(outcome.records, id: \.self) { record in
                        Text(verbatim: record.label)
                            .font(.subheadline)
                            .padding(12)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .surfaceCard(radius: 12)
                    }
                }
            }
            ForEach(outcome.followups) { suggestion in
                if let followUp = suggestion.followUp {
                    FollowUpCard(followUp: followUp,
                                 decision: decisions[suggestion.callId] ?? outcome.decision(for: suggestion),
                                 decide: { accept in
                                     let decision = try await app.decideFollowUp(turnId: outcome.turn.id, callId: suggestion.callId,
                                                                                 followUp, accept: accept)
                                     decisions[suggestion.callId] = decision
                                 },
                                 openAgent: { agentId in showingAgent = AgentSelection(id: agentId, title: followUp.title) })
                }
            }
            if !outcome.steps.isEmpty || !outcome.sources.isEmpty {
                how(outcome)
            }
            foot(outcome)
        }
    }

    /// What was asked, where it stands, and the headline.
    private func head(_ outcome: AskOutcome) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 6) {
                stateLabel(outcome.state)
                if let finished = outcome.turn.finishedAt {
                    Text(verbatim: "·")
                    Text(finished, format: .dateTime.hour().minute().locale(L10n.locale))
                }
                if let agent = outcome.agent, !agent.title.isEmpty {
                    Text(verbatim: "·")
                    Text(verbatim: agent.title).lineLimit(1)
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
            Text(verbatim: L10n.format("You asked: %@", outcome.turn.prompt))
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(4)
                .textSelection(.enabled)
            Text(verbatim: outcome.headline)
                .font(.title2.bold())
                .textSelection(.enabled)
            if outcome.laterTurns > 0 {
                Text(verbatim: outcome.laterTurns == 1
                     ? L10n.string("Continued in 1 later message")
                     : L10n.format("Continued in %lld later messages", outcome.laterTurns))
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder
    private func stateLabel(_ state: TimelineState) -> some View {
        switch state {
        case .done: Label("Done", systemImage: "checkmark.circle").labelStyle(.titleAndIcon)
        case .failed: Label("Failed", systemImage: "xmark.octagon").labelStyle(.titleAndIcon).foregroundStyle(.red)
        case .stopped: Label("Stopped", systemImage: "stop.circle").labelStyle(.titleAndIcon)
        case .asking, .waiting:
            Label("Needs answer", systemImage: "questionmark.bubble").labelStyle(.titleAndIcon).foregroundStyle(Theme.clay)
        case .working: Label("Running", systemImage: "circle.dotted").labelStyle(.titleAndIcon)
        }
    }

    /// A titled part of the outcome.
    private func part<Content: View>(_ title: String, @ViewBuilder content: () -> Content) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(verbatim: title)
                .font(.caption.weight(.semibold))
                .textCase(.uppercase)
                .foregroundStyle(.secondary)
            content()
        }
    }

    /// Something the run remembered, with Forget; once forgotten, it says so.
    private func memoryRow(_ memory: AskOutcome.Memory) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            if forgotten.contains(memory.id) {
                Text(verbatim: L10n.format("Forgotten: %@", memory.text))
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            } else {
                Text(verbatim: memory.text)
                    .font(.subheadline)
                    .textSelection(.enabled)
                Spacer(minLength: 4)
                Button("Forget") { forget(memory) }
                    .buttonStyle(.bordered)
                    .controlSize(.small)
                    .disabled(busy)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: 12)
    }

    /// How Cedar got here: the run's steps and the sources it read, folded.
    private func how(_ outcome: AskOutcome) -> some View {
        let failed = outcome.steps.filter(\.isError).count
        var counts = [outcome.steps.count == 1 ? L10n.string("1 step") : L10n.format("%lld steps", outcome.steps.count)]
        if !outcome.sources.isEmpty {
            counts.append(outcome.sources.count == 1 ? L10n.string("1 source") : L10n.format("%lld sources", outcome.sources.count))
        }
        if failed > 0 {
            counts.append(L10n.format("%lld failed", failed))
        }
        let summary = counts.joined(separator: " · ")
        return DisclosureGroup(isExpanded: $showingHow) {
            VStack(alignment: .leading, spacing: 6) {
                ForEach(Array(outcome.steps.enumerated()), id: \.offset) { index, step in
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(verbatim: "\(index + 1).")
                            .monospacedDigit()
                            .foregroundStyle(.secondary)
                        Text(verbatim: step.name)
                            .font(.footnote.monospaced())
                        if step.isError {
                            Text("Failed")
                                .font(.caption)
                                .foregroundStyle(.red)
                        }
                    }
                    .font(.footnote)
                }
                if !outcome.sources.isEmpty {
                    Text("Sources")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                        .padding(.top, 6)
                    ForEach(Array(outcome.sources.enumerated()), id: \.offset) { _, source in
                        if let link = source.link {
                            Link(destination: link) {
                                Text(verbatim: source.label)
                                    .font(.footnote)
                                    .multilineTextAlignment(.leading)
                            }
                        } else {
                            Text(verbatim: source.label)
                                .font(.footnote)
                        }
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.top, 6)
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                Text("How Cedar got here")
                    .font(.subheadline.weight(.medium))
                Text(verbatim: summary)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(14)
        .surfaceCard()
    }

    /// Keep going: the conversation, where the next message (or the answer to its question) goes.
    private func foot(_ outcome: AskOutcome) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                open(outcome.conversation.id)
            } label: {
                Text(verbatim: L10n.string(outcome.state == .asking ? "Answer" : "Keep going from here"))
            }
            .buttonStyle(ProminentButtonStyle())
            Text("Opens the whole conversation, where you send the next message.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(.top, 4)
    }

    // MARK: Actions

    private func load() async {
        do {
            let fresh = try await app.outcome(turnId: turnId)
            outcome = fresh
            error = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch let error as ChatError where error.status == 404 {
            if outcome == nil { self.error = L10n.string("This conversation no longer exists.") }
        } catch {
            // What is on screen stays; a first load that fails says why.
            if outcome == nil { self.error = error.localizedDescription }
        }
    }

    /// Opens the conversation in the detail column, under this sheet.
    private func open(_ conversationId: String) {
        app.open(conversationId: conversationId)
        dismiss()
    }

    private func forget(_ memory: AskOutcome.Memory) {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.forgetMemory(id: memory.id)
                forgotten.insert(memory.id)
            } catch is CancellationError {
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func stop() {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.stop(turnId: turnId)
                await load()
            } catch is CancellationError {
            } catch {
                actionError = error.localizedDescription
            }
        }
    }
}
