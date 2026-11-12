import SwiftUI

/// Contribute this conversation: the review the web chat's dialog shows, in native form. The
/// conversation as it would leave the server (prompts, the agent's text and tool calls, with the
/// stand-ins the redaction wrote listed), why it can't go if it can't, what to include, a thumb
/// per response, the outcome, and Send. What is sent is the bundle the server previewed, with
/// the outcome and ratings put in (`ContributionPreview.bundle(outcome:notes:ratings:)`).
struct ContributeView: View {
    let conversationId: String
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var preview: ContributionPreview?
    @State private var error: String?
    @State private var reasoning = false
    @State private var fetched = false
    @State private var outcome: String?
    @State private var notes = ""
    @State private var ratings: [Int: Int?] = [:]
    @State private var sending = false
    @State private var sentReceipt: String?

    var body: some View {
        NavigationStack {
            List {
                if let error {
                    Section { Label(error, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(.secondary) }
                }
                if let sentReceipt {
                    Section { Text(L10n.format("Sent. Receipt %@; you can withdraw it any time under Settings → Contribute.", sentReceipt)).foregroundStyle(Theme.green) }
                }
                if let preview {
                    Section {
                        Text(summary(preview)).font(.footnote).foregroundStyle(.secondary)
                        ForEach(preview.blocked, id: \.self) { reason in
                            Text(blockedText(reason, preview)).font(.footnote).foregroundStyle(.red)
                        }
                        if !preview.canSend {
                            Text("This server has no corpus to send to, so nothing can be sent from here.").font(.footnote).foregroundStyle(.secondary)
                        } else if !preview.accepted {
                            Text("Contributing is off. Read the explanation and turn it on under Settings → Contribute before sending.").font(.footnote).foregroundStyle(.secondary)
                        }
                        Text("This is exactly what would leave your server. Read it: the values listed as stand-ins are made up for what was redacted, the same stand-in for the same thing throughout. To redact more, use the web chat.")
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    Section {
                        Toggle("Include the model's reasoning", isOn: $reasoning)
                        Toggle("Include the pages and files the agent fetched", isOn: $fetched)
                    }
                    Section {
                        Picker("Did the agent do what you asked?", selection: $outcome) {
                            Text("Not said").tag(String?.none)
                            Text("Yes").tag(String?.some("succeeded"))
                            Text("Partly").tag(String?.some("partly"))
                            Text("No").tag(String?.some("failed"))
                        }
                        TextField("What was wrong? (optional)", text: $notes, axis: .vertical).lineLimit(2...4)
                    } header: {
                        Text("Outcome")
                    }
                    ForEach(preview.turns()) { turn in
                        Section {
                            turnBody(turn, preview)
                        } header: {
                            HStack {
                                Text(verbatim: L10n.format("Turn %lld", turn.seq))
                                Text(verbatim: "· \(turn.status) · \(turn.day)").foregroundStyle(.secondary)
                                Spacer()
                                thumb(turn, 1, "hand.thumbsup", "Good response")
                                thumb(turn, -1, "hand.thumbsdown", "Bad response")
                            }
                        }
                    }
                    if preview.standInCount > 0 {
                        Section {
                            ForEach(preview.standIns.keys.sorted(), id: \.self) { kind in
                                Text(verbatim: "\(kind): \((preview.standIns[kind] ?? []).joined(separator: ", "))")
                                    .font(.footnote.monospaced()).foregroundStyle(.secondary)
                            }
                        } header: {
                            Text("Stand-ins in this contribution (made up, not real)")
                        }
                    }
                } else if error == nil {
                    ProgressView().frame(maxWidth: .infinity)
                }
            }
            .navigationTitle("Contribute this conversation")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Close") { dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    if let preview, preview.canSend {
                        Button(sending ? "Sending…" : "Send") { send(preview) }
                            .disabled(sending || !preview.accepted || !preview.blocked.isEmpty || sentReceipt != nil)
                    }
                }
            }
            .task(id: "\(reasoning)-\(fetched)") { await load() }
        }
    }

    private func load() async {
        do {
            preview = try await app.contributionPreview(conversationId: conversationId, reasoning: reasoning, fetched: fetched)
            error = nil
        } catch is CancellationError {
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func send(_ preview: ContributionPreview) {
        sending = true
        Task {
            defer { sending = false }
            do {
                let receipt = try await app.sendContribution(conversationId: conversationId, bundle: preview.bundle(outcome: outcome, notes: notes, ratings: ratings))
                sentReceipt = receipt.id
            } catch {
                self.error = error.localizedDescription
            }
        }
    }

    private func rating(_ turn: ContributionPreview.TurnSummary) -> Int? {
        if let chosen: Int? = ratings[turn.seq] { return chosen }
        return turn.rating
    }

    @ViewBuilder
    private func thumb(_ turn: ContributionPreview.TurnSummary, _ value: Int, _ symbol: String, _ label: LocalizedStringKey) -> some View {
        let on = rating(turn) == value
        Button {
            guard let turnId = preview?.turnIds[safe: turn.seq - 1] else { return }
            let next: Int? = on ? nil : value
            Task {
                do {
                    try await app.rateTurn(turnId: turnId, rating: next)
                    ratings[turn.seq] = .some(next)
                } catch {
                    self.error = error.localizedDescription
                }
            }
        } label: {
            Image(systemName: on ? "\(symbol).fill" : symbol)
        }
        .buttonStyle(.plain)
        .foregroundStyle(on ? (value > 0 ? Theme.green : Color.red) : Color.secondary)
        .accessibilityLabel(label)
    }

    @ViewBuilder
    private func turnBody(_ turn: ContributionPreview.TurnSummary, _ preview: ContributionPreview) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("You").font(.caption).foregroundStyle(.secondary)
            Text(turn.prompt).font(.footnote.monospaced())
        }
        VStack(alignment: .leading, spacing: 4) {
            Text("Agent").font(.caption).foregroundStyle(.secondary)
            ForEach(Array(turn.toolCalls.enumerated()), id: \.offset) { _, call in
                Text(verbatim: "\(call.name) \(call.input.prefix(300))").font(.footnote.monospaced()).foregroundStyle(.secondary)
            }
            ForEach(Array(turn.agentText.enumerated()), id: \.offset) { _, text in
                Text(text).font(.footnote.monospaced())
            }
        }
        let marked: [String] = standInsIn(turn, preview)
        if !marked.isEmpty {
            Text(L10n.format("Stand-ins here: %@", marked.joined(separator: ", "))).font(.caption).foregroundStyle(Theme.green)
        }
    }

    /// The stand-ins that occur in this turn's text.
    private func standInsIn(_ turn: ContributionPreview.TurnSummary, _ preview: ContributionPreview) -> [String] {
        var out: [String] = []
        for (_, values) in preview.standIns {
            for value in values {
                var found = turn.prompt.contains(value)
                if !found {
                    for text in turn.agentText where text.contains(value) { found = true; break }
                }
                if !found {
                    for call in turn.toolCalls where call.input.contains(value) { found = true; break }
                }
                if found { out.append(value) }
            }
        }
        return out
    }

    private func summary(_ p: ContributionPreview) -> String {
        var parts: [String] = []
        let turns: Int = p.turns().count
        let turnsKey: String = turns == 1 ? "%lld turn in the bundle" : "%lld turns in the bundle"
        parts.append(L10n.format(turnsKey, turns))
        parts.append(p.modelName)
        let standIns: Int = p.standInCount
        let standInsKey: String = standIns == 1 ? "%lld stand-in" : "%lld stand-ins"
        parts.append(L10n.format(standInsKey, standIns))
        let dropped: JSONValue? = p.dropped
        if dropped?["memory"]?.boolValue == true { parts.append(L10n.string("memory list removed")) }
        let attachments: Int = dropped?["attachments"]?.intValue ?? 0
        if attachments > 0 {
            let key: String = attachments == 1 ? "%lld attachment left out" : "%lld attachments left out"
            parts.append(L10n.format(key, attachments))
        }
        let fetched: Int = dropped?["fetched_bodies"]?.intValue ?? 0
        if fetched > 0 {
            let key: String = fetched == 1 ? "%lld fetched page left out" : "%lld fetched pages left out"
            parts.append(L10n.format(key, fetched))
        }
        let reasoning: Int = dropped?["reasoning"]?.intValue ?? 0
        if reasoning > 0 {
            let key: String = reasoning == 1 ? "reasoning of %lld call left out" : "reasoning of %lld calls left out"
            parts.append(L10n.format(key, reasoning))
        }
        parts.append(L10n.string(p.blocked.isEmpty ? "ready" : "can't be sent"))
        return parts.joined(separator: " · ")
    }

    private func blockedText(_ reason: String, _ p: ContributionPreview) -> String {
        switch reason {
        case "scheduled": return L10n.string("An agent's run can't be contributed.")
        case "active": return L10n.string("Wait for the current response to finish, or stop it.")
        case "no_turns": return L10n.string("There is no finished response to contribute yet.")
        case "private_tools": return L10n.format("This conversation used %@, whose results are other people's data, so it can't be contributed.", p.privateTools.joined(separator: ", "))
        case "run_gone": return L10n.string("A response's run is no longer on the cedar server, so the conversation can't be rebuilt.")
        case "secrets": return L10n.string("A secret is still in the text. Redact it in the web chat.") + "\n" + p.secrets.joined(separator: "\n")
        case "not_consistent": return L10n.string("The bundle fails the corpus's checks:") + "\n" + p.problems.joined(separator: "\n")
        case "model_not_allowed": return L10n.format("The corpus doesn't take conversations from this model (%@).", p.modelName)
        default: return reason
        }
    }
}

private extension Array {
    subscript(safe index: Int) -> Element? { indices.contains(index) ? self[index] : nil }
}
