import SwiftUI

/// The sidebar's Scheduled list: each scheduled agent with its timing and last run, expanding
/// to its runs. A run opens as a conversation in the detail column, as a chat does.
struct ScheduleRows: View {
    @Environment(AppModel.self) private var app
    @Binding var actionError: String?
    @Binding var showingPrompt: Schedule?

    var body: some View {
        if let error = app.scheduleError {
            Section {
                Label(error, systemImage: "exclamationmark.triangle")
                    .foregroundStyle(.secondary)
                    .font(.footnote)
            }
        }
        ForEach(app.schedules) { schedule in
            DisclosureGroup(isExpanded: expanded(schedule)) {
                runs(of: schedule)
            } label: {
                ScheduleRow(schedule: schedule, unread: app.unreadSchedules.contains(schedule.id))
                    .swipeActions {
                        Button {
                            setEnabled(schedule, !schedule.enabled)
                        } label: {
                            schedule.enabled
                                ? Label("Pause", systemImage: "pause")
                                : Label("Resume", systemImage: "play")
                        }
                        .tint(.orange)
                        Button {
                            runNow(schedule)
                        } label: {
                            Label("Run Now", systemImage: "play.circle")
                        }
                        .tint(.accentColor)
                    }
                    .contextMenu {
                        Button {
                            runNow(schedule)
                        } label: {
                            Label("Run Now", systemImage: "play.circle")
                        }
                        .disabled(schedule.lastRun?.isActive ?? false)
                        Button {
                            setEnabled(schedule, !schedule.enabled)
                        } label: {
                            schedule.enabled
                                ? Label("Pause", systemImage: "pause")
                                : Label("Resume", systemImage: "play")
                        }
                        Button {
                            showingPrompt = schedule
                        } label: {
                            Label("Show Prompt", systemImage: "text.alignleft")
                        }
                    }
            }
        }
    }

    @ViewBuilder
    private func runs(of schedule: Schedule) -> some View {
        if let runs = app.scheduleRuns[schedule.id] {
            if runs.isEmpty {
                Text("No runs yet")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            }
            ForEach(runs) { run in
                if let conversationId = run.conversationId {
                    RunRow(run: run)
                        .tag(conversationId)
                } else {
                    // It never started: nothing to open.
                    RunRow(run: run)
                }
            }
        } else {
            ProgressView()
                .frame(maxWidth: .infinity)
        }
    }

    private func expanded(_ schedule: Schedule) -> Binding<Bool> {
        Binding(
            get: { app.expandedSchedules.contains(schedule.id) },
            set: { open in
                if open {
                    app.expandedSchedules.insert(schedule.id)
                    Task { await app.loadRuns(scheduleId: schedule.id) }
                } else {
                    app.expandedSchedules.remove(schedule.id)
                }
            }
        )
    }

    private func runNow(_ schedule: Schedule) {
        Task {
            do {
                try await app.runNow(schedule)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func setEnabled(_ schedule: Schedule, _ enabled: Bool) {
        Task {
            do {
                try await app.setEnabled(schedule, enabled)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }
}

private struct ScheduleRow: View {
    let schedule: Schedule
    let unread: Bool

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(Color.accentColor)
                .frame(width: 8, height: 8)
                .padding(.top, 7)
                .opacity(unread ? 1 : 0)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(schedule.title)
                        .lineLimit(1)
                        .font(unread ? .body.weight(.semibold) : .body)
                    if schedule.lastRun?.isActive ?? false {
                        ProgressView()
                            .controlSize(.mini)
                    }
                }
                Group {
                    Text(schedule.cadence)
                        .lineLimit(1)
                    HStack(spacing: 4) {
                        if schedule.enabled {
                            Text("Next \(schedule.nextRunAt, format: .relative(presentation: .named))")
                        } else {
                            Label("Paused", systemImage: "pause.circle")
                                .labelStyle(.titleAndIcon)
                        }
                        if let last = schedule.lastRun {
                            Text("·")
                            RunOutcomeLabel(outcome: last.outcome)
                        }
                    }
                    .lineLimit(1)
                }
                .font(.caption)
                .foregroundStyle(.secondary)
            }
            .opacity(schedule.enabled ? 1 : 0.7)
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
        .accessibilityValue(unread ? "Unread" : "")
    }
}

private struct RunRow: View {
    let run: ScheduleRun

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Circle()
                .fill(Color.accentColor)
                .frame(width: 6, height: 6)
                .padding(.top, 7)
                .opacity(run.unread ? 1 : 0)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 2) {
                Text(run.dueAt, format: .dateTime.weekday(.abbreviated).month(.abbreviated).day().hour().minute())
                    .font(run.unread ? .subheadline.weight(.semibold) : .subheadline)
                HStack(spacing: 4) {
                    RunOutcomeLabel(outcome: run.outcome)
                    if run.manual { Text("· Run now") }
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

private struct RunOutcomeLabel: View {
    let outcome: ScheduleRun.Outcome

    var body: some View {
        switch outcome {
        case .queued: Label("Queued", systemImage: "clock").labelStyle(.titleAndIcon)
        case .running: Label("Running", systemImage: "circle.dotted").labelStyle(.titleAndIcon)
        case .completed: Label("Done", systemImage: "checkmark.circle").labelStyle(.titleAndIcon)
        case .needsAnswer:
            Label("Needs answer", systemImage: "questionmark.bubble")
                .labelStyle(.titleAndIcon)
                .foregroundStyle(.orange)
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

/// A schedule's prompt, read-only: editing it is done on the web.
struct SchedulePromptView: View {
    let schedule: Schedule
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 12) {
                    Text(schedule.cadence)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                    Text(schedule.prompt)
                        .textSelection(.enabled)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
            }
            .navigationTitle(schedule.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }
}
