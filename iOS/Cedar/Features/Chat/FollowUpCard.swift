import SwiftUI

/// A follow-up the model suggested (`suggest_follow_up`), under the turn that suggested it: what
/// it would do and when, with Set Up (a one-off agent) and Dismiss. Nothing is scheduled until
/// it is set up. Once decided, here or on another device, it says so, and one that was set up
/// opens its agent. A suggestion whose time has passed can only be dismissed.
struct FollowUpCard: View {
    let followUp: FollowUp
    /// What was decided already; `nil` while it is open.
    let decision: FollowUpDecision?
    /// Sets it up (`true`) or dismisses it; throws the server's reason when it refuses.
    let decide: (Bool) async throws -> Void
    /// Opens the agent it was set up as.
    let openAgent: (String) -> Void
    @State private var expanded = false
    @State private var busy = false
    @State private var error: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Suggested Follow-Up", systemImage: "calendar.badge.plus")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(Theme.green)
            if !followUp.title.isEmpty {
                Text(verbatim: followUp.title)
                    .font(.subheadline.weight(.medium))
            }
            Label {
                // In this device's time zone.
                if let date = followUp.date {
                    Text(date, format: .dateTime.weekday(.abbreviated).day().month(.abbreviated).year().hour().minute().locale(L10n.locale))
                } else {
                    Text(verbatim: followUp.at)
                }
            } icon: {
                Image(systemName: "clock")
            }
            .font(.footnote)
            .foregroundStyle(.secondary)
            DisclosureGroup(isExpanded: $expanded) {
                Text(verbatim: followUp.prompt)
                    .font(.footnote)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.top, 4)
            } label: {
                Text("Prompt")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
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

    /// Set Up and Dismiss, while nothing is decided.
    @ViewBuilder
    private var choices: some View {
        let past = followUp.isPast()
        if past {
            Text("This time has passed, so the suggestion can only be dismissed.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        HStack(spacing: 8) {
            Button("Set Up") { send(accept: true) }
                .buttonStyle(.borderedProminent)
                .disabled(busy || past)
            Button("Dismiss Suggestion") { send(accept: false) }
                .buttonStyle(.bordered)
                .disabled(busy)
            if busy { ProgressView().controlSize(.small) }
        }
        .padding(.top, 4)
    }

    /// How it was decided; one that was set up opens its agent, while that still exists.
    @ViewBuilder
    private func decided(_ decision: FollowUpDecision) -> some View {
        if decision.isAccepted {
            HStack(spacing: 12) {
                Label("Agent set up", systemImage: "checkmark.circle.fill")
                    .foregroundStyle(Theme.green)
                if let agentId = decision.agentId {
                    Button("Open Agent") { openAgent(agentId) }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                }
            }
            .font(.footnote)
        } else if decision.isDismissed {
            Label("Dismissed", systemImage: "xmark.circle")
                .font(.footnote)
                .foregroundStyle(.secondary)
        } else {
            // A status from a newer server, as it names it.
            Text(verbatim: decision.status)
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private func send(accept: Bool) {
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await decide(accept)
            } catch is CancellationError {
            } catch {
                // The server's reasons (the time has passed, too many agents) as it gives them.
                self.error = error.localizedDescription
            }
        }
    }
}
