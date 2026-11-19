import SwiftUI

/// A call that was held for approval instead of made: what it would do, what exactly would go
/// out, and Approve or Discard. Approving makes that call as it was written, with no model
/// involved. It is shown under the turn whose run wanted it (`note` is what the tool result
/// said) and in the Agents list's outbox. What became of the call is the app's latest word from
/// the server: one the service refused says why and can be tried again, one that is settled
/// says how.
struct HeldCallCard: View {
    @Environment(AppModel.self) private var app
    /// The held call's id in `/api/outbox`.
    let id: String
    /// What the tool result said, in a conversation; the outbox list has the call itself.
    var note: HeldNote?
    @State private var expanded = true
    @State private var busy = false
    @State private var error: String?

    private var item: OutboxItem? { app.heldCalls[id] }
    private var isOpen: Bool { item?.isOpen ?? false }
    /// What the call would do, from the outbox when it has it, else from the tool result.
    private var reason: String {
        if let reason = item?.reason, !reason.isEmpty { return reason }
        return note?.reason ?? ""
    }
    private var tool: String {
        if let tool = item?.tool, !tool.isEmpty { return tool }
        return note?.tool ?? ""
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(L10n.string(item == nil || isOpen ? "Waiting for Your Approval" : "Outbox"), systemImage: "tray.and.arrow.up")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(isOpen ? Color.orange : Theme.green)
            if !reason.isEmpty {
                Text(verbatim: reason)
                    .font(.subheadline)
                    .textSelection(.enabled)
            }
            Text(verbatim: tool)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
            if let item {
                details(item)
            } else if app.goneHeldCalls.contains(id) {
                Text("This is no longer in the outbox.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            } else {
                ProgressView().controlSize(.small)
            }
            if let error {
                Label(error, systemImage: "exclamationmark.triangle")
                    .font(.footnote)
                    .foregroundStyle(.red)
                    .textSelection(.enabled)
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard()
        .task(id: id) { await app.loadHeldCall(id: id) }
    }

    @ViewBuilder
    private func details(_ item: OutboxItem) -> some View {
        let fields = item.fields
        if !fields.isEmpty {
            DisclosureGroup(isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(fields) { field in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(verbatim: field.name)
                                .font(.caption.monospaced())
                                .foregroundStyle(.secondary)
                            Text(verbatim: field.value)
                                .font(.footnote)
                                .textSelection(.enabled)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.top, 4)
            } label: {
                Text(verbatim: L10n.string(item.isOpen ? "What will go out" : "What it was"))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
        }
        if item.isFailed, let output = item.output, !output.isEmpty {
            Label(L10n.format("It could not be sent: %@", output), systemImage: "exclamationmark.triangle")
                .font(.footnote)
                .foregroundStyle(.red)
                .textSelection(.enabled)
        }
        if item.isOpen {
            Text(verbatim: L10n.string(item.timedOut
                 ? "Nobody answered in time, so this was not done. It goes out exactly as shown when you approve it. To change it first, open Cedar in a browser."
                 : "Nobody was there to approve this, so it was not done. It goes out exactly as shown when you approve it. To change it first, open Cedar in a browser."))
                .font(.caption)
                .foregroundStyle(.secondary)
            HStack(spacing: 8) {
                Button(L10n.string(item.isFailed ? "Try Again" : "Approve")) { decide(approve: true) }
                    .buttonStyle(.borderedProminent)
                    .disabled(busy)
                Button("Discard") { decide(approve: false) }
                    .buttonStyle(.bordered)
                    .disabled(busy)
                if busy { ProgressView().controlSize(.small) }
            }
            .padding(.top, 4)
        } else {
            settled(item)
        }
    }

    /// What became of it, and when.
    @ViewBuilder
    private func settled(_ item: OutboxItem) -> some View {
        let when = item.decidedAt.map { $0.formatted(Date.FormatStyle(date: .abbreviated, time: .shortened).locale(L10n.locale)) } ?? ""
        if item.isSent {
            Label(L10n.format(item.edited ? "Edited, approved and sent · %@" : "Approved and sent · %@", when), systemImage: "checkmark.circle.fill")
                .font(.footnote)
                .foregroundStyle(Theme.green)
        } else if item.isDiscarded {
            Label(L10n.format("Discarded · %@", when), systemImage: "xmark.circle")
                .font(.footnote)
                .foregroundStyle(.secondary)
        } else {
            // Being sent right now, or a status from a newer server, as it names it.
            Text(verbatim: item.status)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private func decide(approve: Bool) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await app.decideHeldCall(id: id, approve: approve)
            } catch is CancellationError {
            } catch {
                // The server's reason (it was decided elsewhere, the account is disconnected).
                self.error = error.localizedDescription
            }
        }
    }
}
