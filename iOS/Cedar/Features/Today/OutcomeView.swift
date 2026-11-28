import SwiftUI

/// What one ask came to, in a sheet: the answer rather than the transcript. What was asked, the
/// gist as the headline, the answer, then what the turn left behind (the files it wrote or
/// changed, what it remembered, the record rows it saved, the follow-up it suggested, what it
/// held for your OK), with its steps and sources folded underneath. Keep going sends the next
/// message from here and opens the conversation, where the reply streams.
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
    /// The next message, typed under the outcome.
    @State private var next = ""
    @State private var sending = false

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
            .background(TodayColors.page)
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
            // Without the paragraph the headline already says.
            if let answer = outcome.answer {
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
                        recordRow(record)
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
            Label("Needs answer", systemImage: "questionmark.bubble").labelStyle(.titleAndIcon).foregroundStyle(TodayColors.needText)
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

    /// A record row the run saved: its set and key, and its values underneath.
    private func recordRow(_ record: AskOutcome.Record) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(verbatim: record.label)
                .font(.subheadline.weight(.medium))
            ForEach(record.values(), id: \.self) { value in
                Text(verbatim: value)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(radius: 12)
        .textSelection(.enabled)
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
                        // "Web search", not `web_search`, as the chat names its steps.
                        Text(verbatim: stepLabel(step.name))
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

    /// Keep going: the next message (or the answer to its question), sent from here; the
    /// conversation then opens, where the reply streams. The whole conversation is one tap away.
    private func foot(_ outcome: AskOutcome) -> some View {
        let asking = outcome.state == .asking
        let text = next.trimmingCharacters(in: .whitespacesAndNewlines)
        return VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .bottom, spacing: 8) {
                TextField(L10n.string(asking ? "Your answer" : "Keep going from here"), text: $next, axis: .vertical)
                    .lineLimit(1...5)
                    .textFieldStyle(.roundedBorder)
                    .disabled(sending)
                if sending {
                    ProgressView()
                } else {
                    Button {
                        keepGoing(outcome.conversation.id, text)
                    } label: {
                        Text(verbatim: L10n.string(asking ? "Answer" : "Send"))
                            .foregroundStyle(TodayColors.primaryInk)
                    }
                    .buttonStyle(.borderedProminent)
                    .tint(TodayColors.primaryFill)
                    // A response still going takes no new message.
                    .disabled(text.isEmpty || outcome.state == .working)
                }
            }
            Button("Open the Whole Conversation") { open(outcome.conversation.id) }
                .buttonStyle(.borderless)
                .font(.footnote)
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

    /// Sends the next message; the conversation opens under this sheet as it starts.
    private func keepGoing(_ conversationId: String, _ text: String) {
        guard !text.isEmpty else { return }
        sending = true
        Task {
            defer { sending = false }
            do {
                try await app.keepGoing(conversationId: conversationId, text: text)
                next = ""
                dismiss()
            } catch is CancellationError {
            } catch {
                actionError = error.localizedDescription
            }
        }
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
