import SwiftUI

/// Settings → Contribute: what a contribution contains, the one-time choices (license, pseudonym,
/// what to include by default), the switch that turns contributing on, and the receipts with
/// what the corpus says about each and Withdraw. Choices save at once, like Memory's deletions.
struct ContributeSettingsView: View {
    @Environment(AppModel.self) private var app
    @State private var actionError: String?
    @State private var withdrawing: Contribution?
    @State private var refreshing = false

    var body: some View {
        List {
            if let error = app.contributionsError {
                Section { Label(error, systemImage: "exclamationmark.triangle").font(.footnote).foregroundStyle(.secondary) }
            }
            if let s = app.contributionSettings {
                Section {
                    Text("Give single conversations to a shared corpus for training open-weight agent models. Opt-in, one conversation at a time, after you read exactly what goes.")
                    if !s.available {
                        Text("Whoever runs this server has not named a corpus, so nothing can be sent from here.").foregroundStyle(.secondary)
                    }
                    if let host = s.host {
                        Text(L10n.format("Corpus: %@", host)).foregroundStyle(.secondary)
                    }
                }
                Section {
                    Text("Your prompts, the agent's replies and tool calls, the system prompt and tool definitions it ran with, how the run ended, and the outcome you report.")
                    Text("Never: attached files and their names, your memory list, run and key identifiers, exact times, your region. Emails, phone and account numbers, IP addresses, keys and home-directory names become made-up stand-ins of the same shape, consistent within the conversation and random otherwise.")
                    Text("Conversations that used Gmail or Calendar can't be contributed. Redaction is best effort: the review is where you read for what it can't catch.")
                    Text("No account, no identity. A pseudonym, if you choose one, is a random id made on your server; you can rotate or drop it. You can withdraw a contribution below.")
                } header: {
                    Text("What a contribution contains")
                }
                .font(.footnote)
                if s.available {
                    Section {
                        Picker("License for what you wrote", selection: Binding(
                            get: { s.license },
                            set: { value in save(ContributionSettingsPatch(license: value)) }
                        )) {
                            ForEach(s.licenses, id: \.self) { id in
                                Text(id == "cc0" ? "CC0 (public domain)" : "CC BY 4.0 (credit by pseudonym)").tag(id)
                            }
                        }
                        LabeledContent("Pseudonym") {
                            Text(verbatim: s.pseudonym ?? L10n.string("none (unlinked)")).font(.footnote.monospaced())
                        }
                        HStack {
                            Button("New pseudonym") { save(ContributionSettingsPatch(pseudonym: "new")) }
                            if s.pseudonym != nil {
                                Spacer()
                                Button("Drop", role: .destructive) { save(ContributionSettingsPatch(pseudonym: "")) }
                            }
                        }
                        .buttonStyle(.borderless)
                        Toggle("Include the model's reasoning by default", isOn: Binding(
                            get: { s.includeReasoning },
                            set: { on in save(ContributionSettingsPatch(includeReasoning: on)) }
                        ))
                        Toggle("Include fetched pages by default", isOn: Binding(
                            get: { s.includeFetched },
                            set: { on in save(ContributionSettingsPatch(includeFetched: on)) }
                        ))
                        Toggle("Contributing is on", isOn: Binding(
                            get: { s.enabled },
                            set: { on in save(ContributionSettingsPatch(enabled: on)) }
                        ))
                    } footer: {
                        Text("Links your contributions to each other, and to nothing else; without a pseudonym, each stands alone. Turning contributing on only makes Send available in the review. Nothing is ever sent on its own.")
                    }
                }
                Section {
                    if app.contributions.isEmpty {
                        Text("Nothing sent yet.").foregroundStyle(.secondary)
                    }
                    ForEach(app.contributions) { c in
                        receiptRow(c)
                    }
                } header: {
                    HStack {
                        Text("Your contributions")
                        Spacer()
                        if s.available, app.contributions.contains(where: { $0.withdrawnAt == nil }) {
                            Button(refreshing ? "Refreshing…" : "Refresh status") {
                                refreshing = true
                                Task {
                                    await app.refreshContributions(askCorpus: true)
                                    refreshing = false
                                }
                            }
                            .disabled(refreshing)
                            .font(.footnote)
                        }
                    }
                }
            } else if app.contributionsError == nil {
                ProgressView().frame(maxWidth: .infinity)
            }
        }
        .navigationTitle("Contribute")
        .task { await app.refreshContributions() }
        .refreshable { await app.refreshContributions(askCorpus: true) }
        .alert("Withdraw This Contribution?", isPresented: Binding(
            get: { withdrawing != nil },
            set: { if !$0 { withdrawing = nil } }
        )) {
            Button("Withdraw", role: .destructive) {
                if let c = withdrawing {
                    Task {
                        do { try await app.withdrawContribution(id: c.id) } catch { actionError = error.localizedDescription }
                    }
                }
                withdrawing = nil
            }
            Button("Cancel", role: .cancel) { withdrawing = nil }
        } message: {
            Text("It is deleted from the corpus's intake and left out of future releases.")
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
    private func receiptRow(_ c: Contribution) -> some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: receiptTitle(c))
                Text(verbatim: receiptDetail(c))
                    .font(.footnote).foregroundStyle(.secondary)
            }
            Spacer()
            if c.withdrawnAt != nil {
                Text("Withdrawn").font(.footnote).foregroundStyle(.secondary)
            } else {
                if let status = c.status {
                    Text(statusText(status)).font(.footnote).foregroundStyle(status == "released" ? Theme.green : Color.secondary)
                }
                Button("Withdraw", role: .destructive) { withdrawing = c }.buttonStyle(.borderless).font(.footnote)
            }
        }
    }

    private func receiptTitle(_ c: Contribution) -> String {
        let title: String = c.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return title.isEmpty ? L10n.string("Untitled conversation") : title
    }

    private func receiptDetail(_ c: Contribution) -> String {
        let when: String = c.submittedAt.formatted(.relative(presentation: .named).locale(L10n.locale))
        var detail: String = "\(when) · \(c.license) · \(c.id)"
        if !c.releasedIn.isEmpty { detail += " · " + c.releasedIn.joined(separator: ", ") }
        return detail
    }

    private func statusText(_ status: String) -> String {
        switch status {
        case "quarantined": return L10n.string("received, not yet screened")
        case "accepted": return L10n.string("accepted")
        case "held": return L10n.string("held for review")
        case "rejected": return L10n.string("rejected")
        case "released": return L10n.string("released")
        default: return L10n.string("unknown to the corpus")
        }
    }

    private func save(_ patch: ContributionSettingsPatch) {
        Task {
            do { try await app.updateContributionSettings(patch) } catch { actionError = error.localizedDescription }
        }
    }
}
