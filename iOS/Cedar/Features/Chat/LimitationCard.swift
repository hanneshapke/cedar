import SwiftUI

/// Something the model said it could not do (`report_limitation`), under the turn that said it:
/// the sentence, what would help, and Send Feedback or Dismiss, with room for a note. Sending
/// keeps the report on the person's own server; nothing is recorded before that. Once decided,
/// here or on another device, it says which.
struct LimitationCard: View {
    let report: LimitationReport
    /// What was decided already; `nil` while it is open.
    let decision: FeedbackDecision?
    /// Sends it as feedback (`true`, with the note) or dismisses it; throws the server's reason
    /// when it refuses.
    let decide: (Bool, String) async throws -> Void
    @State private var comment = ""
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Cedar Couldn't Do This", systemImage: "hand.raised")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(Theme.green)
            Text(verbatim: report.summary)
                .font(.subheadline)
                .textSelection(.enabled)
            Text(verbatim: Self.label(for: report.category))
                .font(.caption)
                .foregroundStyle(.secondary)
            if let needed = report.needed {
                Text(verbatim: L10n.format("Would need: %@", needed))
                    .font(.footnote)
            }
            if let decision {
                decided(decision)
            } else {
                choices
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
    }

    /// The words for a report's category; one this app doesn't know reads as "other".
    static func label(for category: String) -> String {
        switch category {
        case "missing_tool": return L10n.string("No tool for it")
        case "not_connected": return L10n.string("Not connected")
        case "blocked": return L10n.string("Blocked")
        case "limit": return L10n.string("Hit a limit")
        default: return L10n.string("Other")
        }
    }

    /// The note, Send Feedback and Dismiss, while nothing is decided.
    @ViewBuilder
    private var choices: some View {
        TextField("Add a note (optional)", text: $comment, axis: .vertical)
            .lineLimit(1...4)
            .textFieldStyle(.roundedBorder)
            .font(.footnote)
            .disabled(busy)
        Text("Nothing is recorded unless you send it. Feedback stays on your server, for whoever runs it.")
            .font(.caption)
            .foregroundStyle(.secondary)
        HStack(spacing: 8) {
            Button("Send Feedback") { send(true) }
                .buttonStyle(.borderedProminent)
                .disabled(busy)
            Button("Dismiss Report") { send(false) }
                .buttonStyle(.bordered)
                .disabled(busy)
            if busy { ProgressView().controlSize(.small) }
        }
        .padding(.top, 4)
    }

    @ViewBuilder
    private func decided(_ decision: FeedbackDecision) -> some View {
        if decision.isSent {
            Label("Sent as feedback", systemImage: "checkmark.circle.fill")
                .font(.footnote)
                .foregroundStyle(Theme.green)
        } else if decision.isDismissed {
            Label("Report Dismissed", systemImage: "xmark.circle")
                .font(.footnote)
                .foregroundStyle(.secondary)
        } else {
            // A status from a newer server, as it names it.
            Text(verbatim: decision.status)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private func send(_ feedback: Bool) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await decide(feedback, String(comment.prefix(1000)))
            } catch is CancellationError {
            } catch {
                self.error = error.localizedDescription
            }
        }
    }
}
