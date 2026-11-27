import SwiftUI

/// Which ask's outcome to show, in a sheet.
struct OutcomeSelection: Identifiable, Hashable {
    /// The turn's id.
    let id: String
}

/// The sidebar's Today list: one day as a river. What is going or waits on you comes first
/// (Now): a risky call is allowed or denied right there, a held call approved or discarded, and
/// a plan or a question opens its conversation. Earlier is what Cedar did today, newest first,
/// each opening its outcome; Ahead is the agents due next and the ones that keep watch.
struct TodayRows: View {
    @Environment(AppModel.self) private var app
    @Binding var actionError: String?
    /// Opens an ask's outcome, by its turn's id.
    let showOutcome: (String) -> Void
    /// Opens an agent's detail.
    let showAgent: (TimelineAgent) -> Void
    /// Opens the sheet to ask with a When.
    let ask: () -> Void

    var body: some View {
        Section {
            Button {
                ask()
            } label: {
                Label("Ask for Something", systemImage: "square.and.pencil")
            }
            .buttonStyle(ProminentButtonStyle())
            .disabled(!app.settings.isConfigured)
            .listRowSeparator(.hidden)
            .sidebarRow()
        }
        if let error = app.todayError {
            Section {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .font(.footnote)
                    .sidebarRow()
            }
        }
        if let timeline = app.timeline {
            river(timeline)
        } else if app.todayError == nil {
            ProgressView()
                .frame(maxWidth: .infinity)
                .listRowSeparator(.hidden)
                .sidebarRow()
        }
    }

    @ViewBuilder
    private func river(_ timeline: Timeline) -> some View {
        if timeline.isEmpty {
            ContentUnavailableView("Nothing Yet Today", systemImage: "calendar",
                                   description: Text("Ask for something, and it lands here."))
                .listRowSeparator(.hidden)
                .sidebarRow()
        }
        if !timeline.now.isEmpty || !timeline.outbox.isEmpty {
            Section {
                ForEach(timeline.now) { turn in
                    TodayNowRow(turn: turn, actionError: $actionError)
                        .listRowSeparator(.hidden)
                        .sidebarRow()
                }
                ForEach(timeline.outbox) { held in
                    heldRow(held)
                        .listRowSeparator(.hidden)
                        .sidebarRow()
                }
            } header: {
                HStack {
                    Text("Now")
                    Spacer()
                    if timeline.needsYou > 0 {
                        Text(verbatim: timeline.needsYou == 1
                             ? L10n.string("1 thing needs you")
                             : L10n.format("%lld things need you", timeline.needsYou))
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Theme.clay)
                    }
                }
            }
        }
        if !timeline.earlier.isEmpty {
            Section {
                // Newest first: what just finished is what is looked for.
                ForEach(timeline.earlier.reversed()) { turn in
                    Button {
                        showOutcome(turn.turnId)
                    } label: {
                        TodayEarlierRow(turn: turn)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .sidebarRow()
                }
            } header: {
                Text("Earlier")
            }
        }
        if !timeline.ahead.isEmpty || !timeline.watching.isEmpty {
            Section {
                ForEach(timeline.ahead + timeline.watching) { agent in
                    Button {
                        showAgent(agent)
                    } label: {
                        TodayAheadRow(agent: agent)
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    .sidebarRow()
                }
            } header: {
                Text("Ahead")
            }
        }
    }

    /// A call an unattended run held: who wanted it (opening that run's outcome), and the card
    /// that approves or discards it.
    private func heldRow(_ held: TimelineHeld) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            let origin = held.item.origin ?? L10n.string("Held for your OK")
            if let turnId = held.turnId {
                Button {
                    showOutcome(turnId)
                } label: {
                    Text(verbatim: origin)
                        .font(.footnote.weight(.medium))
                }
                .buttonStyle(.borderless)
            } else {
                Text(verbatim: origin)
                    .font(.footnote.weight(.medium))
                    .foregroundStyle(.secondary)
            }
            HeldCallCard(id: held.item.id)
        }
    }
}

/// A turn under Now: still going, waiting on a go-ahead or an OK, or asking a question.
private struct TodayNowRow: View {
    @Environment(AppModel.self) private var app
    let turn: TimelineTurn
    @Binding var actionError: String?
    @State private var busy = false

    private var needsYou: Bool { turn.state.needsYou }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Button {
                    app.open(conversationId: turn.conversationId)
                } label: {
                    Text(verbatim: turn.displayTitle)
                        .font(.subheadline.weight(.semibold))
                        .multilineTextAlignment(.leading)
                        .lineLimit(3)
                }
                .buttonStyle(.borderless)
                .tint(.primary)
                Spacer(minLength: 4)
                Text(verbatim: pill)
                    .font(.caption2.weight(.semibold))
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background((needsYou ? Theme.clay : Theme.green).opacity(0.15), in: Capsule())
                    .foregroundStyle(needsYou ? Theme.clay : Theme.green)
            }
            switch turn.state {
            case .waiting:
                ForEach(turn.waits) { wait in
                    waiting(wait)
                }
            case .asking:
                if let question = turn.summary, !question.isEmpty {
                    Text(verbatim: question)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                Button("Answer") { app.open(conversationId: turn.conversationId) }
                    .buttonStyle(.borderedProminent)
                    .tint(Theme.clay)
            default:
                HStack(spacing: 10) {
                    ProgressView().controlSize(.small)
                    Text("Working on it")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                    Spacer(minLength: 4)
                    Button("Stop") { stop() }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                        .disabled(busy)
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .surfaceCard(stroke: needsYou ? Theme.clay.opacity(0.6) : Theme.line)
    }

    private var pill: String {
        switch turn.state {
        case .waiting: return L10n.string("Waiting for you")
        case .asking: return L10n.string("Cedar asks")
        default: return L10n.string("Working")
        }
    }

    /// A plan opens its conversation, where its steps can be changed; a call is decided here.
    @ViewBuilder
    private func waiting(_ wait: TimelineWait) -> some View {
        if let approvalId = wait.approvalId {
            let reason = (wait.reason ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            Text(verbatim: reason.isEmpty ? L10n.string("Cedar wants to do something that needs your OK.") : reason)
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
            HStack(spacing: 8) {
                Button("Allow") { decide(approvalId, allow: true) }
                    .buttonStyle(.borderedProminent)
                    .tint(Theme.clay)
                    .disabled(busy)
                Button("Deny") { decide(approvalId, allow: false) }
                    .buttonStyle(.bordered)
                    .disabled(busy)
                if busy { ProgressView().controlSize(.small) }
            }
        } else {
            Text("Cedar wrote a plan and waits for your go-ahead.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
            Button("Review the Plan") { app.open(conversationId: turn.conversationId) }
                .buttonStyle(.borderedProminent)
                .tint(Theme.clay)
        }
    }

    private func decide(_ approvalId: String, allow: Bool) {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.decideApproval(turnId: turn.turnId, approvalId: approvalId, allow: allow)
            } catch is CancellationError {
            } catch {
                actionError = error.localizedDescription
                await app.refreshToday()
            }
        }
    }

    private func stop() {
        busy = true
        Task {
            defer { busy = false }
            do {
                try await app.stop(turnId: turn.turnId)
            } catch is CancellationError {
            } catch {
                actionError = error.localizedDescription
                await app.refreshToday()
            }
        }
    }
}

/// A turn under Earlier: when, what, and how it went in one line.
private struct TodayEarlierRow: View {
    let turn: TimelineTurn

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Text(turn.createdAt, format: .dateTime.hour().minute().locale(L10n.locale))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 58, alignment: .leading)
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: turn.displayTitle)
                    .font(.body)
                    .foregroundStyle(Color.primary)
                    .lineLimit(2)
                HStack(spacing: 6) {
                    if let agent = turn.agent, !agent.title.isEmpty, agent.title != turn.displayTitle {
                        Label(agent.title, systemImage: "clock")
                            .labelStyle(.titleAndIcon)
                            .lineLimit(1)
                    }
                    if turn.state == .failed {
                        Text("Failed").foregroundStyle(.red)
                    } else if turn.state == .stopped {
                        Text("Stopped")
                    }
                }
                .font(.caption)
                .foregroundStyle(.secondary)
                if let line = turn.line {
                    Text(verbatim: line)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }
}

/// An agent under Ahead: when it runs next, or "any time" for one that something outside starts,
/// and what starts it.
private struct TodayAheadRow: View {
    let agent: TimelineAgent

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Group {
                if let next = agent.nextRunAt, !agent.watches {
                    if Calendar.current.isDateInToday(next) {
                        Text(next, format: .dateTime.hour().minute().locale(L10n.locale))
                    } else {
                        Text(next, format: .dateTime.weekday(.abbreviated).hour().minute().locale(L10n.locale))
                    }
                } else {
                    Text("any time")
                }
            }
            .font(.caption.monospacedDigit())
            .foregroundStyle(.secondary)
            .frame(width: 58, alignment: .leading)
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: agent.title)
                    .font(.body)
                    .foregroundStyle(Color.primary)
                    .lineLimit(2)
                Label(agent.trigger.summary, systemImage: agent.trigger.systemImage)
                    .labelStyle(.titleAndIcon)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 0)
        }
        .padding(.vertical, 6)
        .accessibilityElement(children: .combine)
    }
}

/// What the Today list presents over the sidebar: an ask's outcome, the sheet to ask with a
/// When, and the periodic refresh while Today is the list on screen.
struct TodayPresentation: ViewModifier {
    @Environment(AppModel.self) private var app
    @Binding var outcome: OutcomeSelection?
    @Binding var asking: Bool
    /// How often the river is read again while it shows.
    private static let refreshSeconds = 20

    func body(content: Content) -> some View {
        content
            .sheet(item: $outcome) { selection in
                OutcomeView(turnId: selection.id)
            }
            .sheet(isPresented: $asking) {
                AskView()
            }
            .task(id: app.sidebarMode) {
                // Cancelled when the list leaves the screen or shows something else.
                while app.sidebarMode == .today, !Task.isCancelled {
                    await app.refreshToday()
                    try? await Task.sleep(for: .seconds(Self.refreshSeconds))
                }
            }
    }
}
