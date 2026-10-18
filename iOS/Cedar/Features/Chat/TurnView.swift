import SwiftUI
import UIKit

/// A message and everything the agent did to answer it.
struct TurnView: View {
    let turn: TurnTranscript
    let chat: ChatModel
    /// The conversation in the web UI, for what the app doesn't draw itself.
    let webURL: URL?
    let isLast: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            UserBubble(text: turn.prompt, attachments: turn.turn.attachments)
            ForEach(turn.items) { item in
                switch item {
                case .text(_, let text):
                    MarkdownView(text: text)
                case .reasoning(_, let text, let durationMs):
                    ReasoningView(text: text, durationMs: durationMs)
                case .plan(_, let plan):
                    PlanView(plan: plan, canAgree: isLast && turn.isActive) {
                        Task { await chat.agree(to: plan, in: turn) }
                    }
                case .approval(let approval):
                    ApprovalView(approval: approval, canDecide: isLast && turn.isActive) { decision in
                        Task { await chat.decide(approval, in: turn, decision: decision) }
                    }
                case .tool(let call):
                    ToolCallView(call: call, webURL: webURL)
                }
            }
            if turn.items.isEmpty, let final = turn.turn.finalText, !final.isEmpty {
                MarkdownView(text: final)
            }
            if !turn.isActive, let conversationId = chat.conversationId, let client = chat.client {
                let images = turn.images
                if !images.isEmpty {
                    ImagesView(paths: images, conversationId: conversationId, client: client)
                }
            }
            StatusFooter(turn: turn)
        }
    }
}

private struct StatusFooter: View {
    let turn: TurnTranscript
    @State private var copied = false

    var body: some View {
        switch turn.turn.status {
        case .queued:
            Label { Text("Queued") } icon: { ProgressView().controlSize(.small) }
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .running:
            Label { Text(turn.pendingApproval != nil ? "Waiting for your say-so" : turn.awaitingPlan != nil ? "Waiting for the plan" : "Working…") } icon: { ProgressView().controlSize(.small) }
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .failed:
            Label(turn.turn.error ?? "The run failed.", systemImage: "exclamationmark.triangle.fill")
                .font(.footnote)
                .foregroundStyle(.red)
                .textSelection(.enabled)
        case .cancelled:
            Label(turn.cancelReason.map { "Stopped: \($0)" } ?? "Stopped", systemImage: "stop.circle")
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .completed:
            HStack(spacing: 12) {
                Button {
                    UIPasteboard.general.string = turn.answer
                    copied = true
                    Task {
                        try? await Task.sleep(for: .seconds(1.5))
                        copied = false
                    }
                } label: {
                    Image(systemName: copied ? "checkmark" : "doc.on.doc")
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Copy answer")
                if turn.usage != .zero {
                    Text("\(turn.usage.inputTokens.formatted()) in · \(turn.usage.outputTokens.formatted()) out")
                }
                if let cost = turn.usage.costUSD {
                    Text(cost, format: .currency(code: "USD").precision(.fractionLength(cost < 0.01 ? 4 : 2)))
                }
                if turn.turn.needsAnswer {
                    Label("Needs your answer", systemImage: "questionmark.bubble")
                        .foregroundStyle(.orange)
                } else if turn.turn.stopReason == "max_tokens" {
                    Text("Cut short")
                }
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }
}

/// The model's reasoning, folded to one line.
private struct ReasoningView: View {
    let text: String
    let durationMs: Int?
    @State private var expanded = false

    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            Text(text)
                .font(.footnote)
                .foregroundStyle(.secondary)
                .textSelection(.enabled)
                .padding(.top, 4)
        } label: {
            Label(title, systemImage: "brain")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private var title: String {
        guard let ms = durationMs, ms > 0 else { return "Thought it through" }
        return "Thought for \(Duration.milliseconds(ms).formatted(.units(allowed: [.seconds], width: .abbreviated)))"
    }
}

/// The steps the agent proposed; "Go ahead" when it waits for them.
private struct PlanView: View {
    let plan: Plan
    let canAgree: Bool
    let agree: () -> Void
    @State private var sent = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(plan.awaiting ? "Plan, waiting for your go-ahead" : plan.by == "timeout" ? "Plan (followed as proposed)" : "Plan", systemImage: "list.number")
                .font(.subheadline.weight(.medium))
            ForEach(Array(plan.steps.enumerated()), id: \.offset) { offset, step in
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("\(offset + 1).")
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                    Text(step.title)
                }
                .font(.subheadline)
            }
            if plan.awaiting && canAgree {
                Button(sent ? "Sent" : "Go ahead") {
                    sent = true
                    agree()
                }
                .buttonStyle(.borderedProminent)
                .disabled(sent)
                .padding(.top, 4)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.secondarySystemBackground).opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(.separator).opacity(0.5)))
    }
}

/// A tool call waiting for the user's say-so, with what it would do.
private struct ApprovalView: View {
    let approval: Approval
    let canDecide: Bool
    let decide: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label(approval.isPending ? "May the agent do this?" : "Asked: \(approval.reason)", systemImage: "hand.raised")
                .font(.subheadline.weight(.medium))
            if approval.isPending {
                Text(approval.reason)
                    .font(.subheadline)
            }
            CodeBox(label: approval.tool, text: summary)
            if approval.isPending {
                if canDecide {
                    HStack(spacing: 8) {
                        Button("Allow") { decide("allow") }
                            .buttonStyle(.borderedProminent)
                        Button(approval.alwaysLabel) { decide("allow_always") }
                            .buttonStyle(.bordered)
                        Button("Deny", role: .destructive) { decide("deny") }
                            .buttonStyle(.bordered)
                    }
                    .padding(.top, 4)
                }
            } else {
                Text(decisionText)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color.orange.opacity(approval.isPending ? 0.12 : 0.05), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color.orange.opacity(approval.isPending ? 0.5 : 0.2)))
    }

    private var summary: String {
        switch approval.tool {
        case "bash": return approval.input["command"]?.stringValue ?? approval.input.prettyPrinted
        case "run_python": return approval.input["code"]?.stringValue ?? approval.input.prettyPrinted
        case "download_file": return approval.input["url"]?.stringValue ?? approval.input.prettyPrinted
        default: return approval.input.prettyPrinted
        }
    }

    private var decisionText: String {
        switch approval.decision {
        case "allow": return "Allowed"
        case "allow_always": return "Allowed, and remembered"
        case "deny": return "Denied"
        case "timeout": return "No answer in time; not done"
        default: return approval.decision ?? ""
        }
    }
}

/// The clarifying questions the agent asked, as a form whose answer is the next message.
struct QuestionsView: View {
    let questions: Questions
    let submit: ([ClarifyAnswer.Answer]) -> Void
    @State private var answers: [ClarifyAnswer.Answer]
    @State private var sent = false

    init(questions: Questions, submit: @escaping ([ClarifyAnswer.Answer]) -> Void) {
        self.questions = questions
        self.submit = submit
        _answers = State(initialValue: Array(repeating: ClarifyAnswer.Answer(), count: questions.questions.count))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            Label("Cedar asks", systemImage: "questionmark.bubble")
                .font(.subheadline.weight(.medium))
            ForEach(Array(questions.questions.enumerated()), id: \.offset) { index, question in
                VStack(alignment: .leading, spacing: 8) {
                    if let header = question.header, !header.isEmpty {
                        Text(header.uppercased())
                            .font(.caption2.weight(.semibold))
                            .foregroundStyle(.secondary)
                    }
                    Text(question.question)
                        .font(.subheadline)
                    ForEach(question.options, id: \.self) { option in
                        let chosen = answers[index].chosen.contains(option.label)
                        Button {
                            toggle(option.label, at: index, multi: question.multiSelect)
                        } label: {
                            HStack(alignment: .firstTextBaseline, spacing: 8) {
                                Image(systemName: chosen ? (question.multiSelect ? "checkmark.square.fill" : "largecircle.fill.circle") : (question.multiSelect ? "square" : "circle"))
                                    .foregroundStyle(chosen ? Color.accentColor : Color.secondary)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.label)
                                    if let description = option.description, !description.isEmpty {
                                        Text(description)
                                            .font(.caption)
                                            .foregroundStyle(.secondary)
                                    }
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                    }
                    TextField(question.options.isEmpty ? "Your answer" : "Other", text: $answers[index].own, axis: .vertical)
                        .lineLimit(1...4)
                        .textFieldStyle(.roundedBorder)
                }
            }
            if let note = questions.note, !note.isEmpty {
                Text(note)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Button(sent ? "Sent" : "Answer") {
                sent = true
                submit(answers)
            }
            .buttonStyle(.borderedProminent)
            .disabled(sent)
        }
        .padding(12)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Color(.secondarySystemBackground).opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(.separator).opacity(0.5)))
    }

    private func toggle(_ label: String, at index: Int, multi: Bool) {
        if multi {
            if let at = answers[index].chosen.firstIndex(of: label) {
                answers[index].chosen.remove(at: at)
            } else {
                answers[index].chosen.append(label)
            }
        } else {
            answers[index].chosen = answers[index].chosen == [label] ? [] : [label]
        }
    }
}

/// A collapsible tool call: a one-line summary, and the input and output when expanded. A
/// card tool (map, recipe, route, stocks) offers the web UI, which draws it.
struct ToolCallView: View {
    let call: ToolCall
    var webURL: URL?
    @State private var expanded = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button {
                withAnimation(.snappy(duration: 0.2)) { expanded.toggle() }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: icon)
                        .frame(width: 18)
                        .foregroundStyle(.secondary)
                    Text(call.name)
                        .font(.subheadline.weight(.medium))
                    Text(call.summary)
                        .font(call.isCard ? .caption : .caption.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                    Spacer(minLength: 4)
                    statusIcon
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.tertiary)
                        .rotationEffect(.degrees(expanded ? 90 : 0))
                }
                .contentShape(Rectangle())
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            }
            .buttonStyle(.plain)

            if expanded {
                Divider()
                VStack(alignment: .leading, spacing: 10) {
                    if call.isCard, call.isFinished, !call.isError, let webURL {
                        Link(destination: webURL) {
                            Label("Open the \(cardKind) on the web", systemImage: "safari")
                                .font(.subheadline)
                        }
                    }
                    CodeBox(label: "Input", text: inputText)
                    if let output = call.output {
                        CodeBox(label: call.isError ? "Error" : "Output",
                                text: output.isEmpty ? "(no output)" : output,
                                isError: call.isError)
                    }
                    if !call.sources.isEmpty {
                        VStack(alignment: .leading, spacing: 2) {
                            ForEach(call.sources, id: \.n) { source in
                                if let url = URL(string: source.url) {
                                    Link("[\(source.n)] \(source.title ?? source.url)", destination: url)
                                        .font(.caption)
                                        .lineLimit(1)
                                }
                            }
                        }
                    }
                    if let ms = call.durationMs {
                        Text(Duration.milliseconds(ms).formatted(.units(allowed: [.seconds, .milliseconds], width: .abbreviated)))
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                }
                .padding(12)
            }
        }
        .background(Color(.secondarySystemBackground).opacity(0.6), in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Color(.separator).opacity(0.5)))
    }

    private var icon: String {
        switch call.name {
        case "bash": return "terminal"
        case "run_python": return "chevron.left.forwardslash.chevron.right"
        case "read_file", "read_document": return "doc.text"
        case "write_file": return "square.and.pencil"
        case "web_search": return "magnifyingglass"
        case "fetch_url", "download_file": return "globe"
        case "generate_chart": return "chart.bar"
        case "generate_pdf": return "doc.richtext"
        case "weather_forecast": return "cloud.sun"
        case "show_map": return "map"
        case "show_recipe": return "fork.knife"
        case "plan_trip": return "tram"
        case "compare_stocks": return "chart.line.uptrend.xyaxis"
        case "ask_user": return "questionmark.bubble"
        case "browser": return "safari"
        default: return "wrench.and.screwdriver"
        }
    }

    private var cardKind: String {
        switch call.name {
        case "show_map": return "map"
        case "show_recipe": return "recipe"
        case "plan_trip": return "route"
        case "compare_stocks": return "comparison"
        default: return "result"
        }
    }

    @ViewBuilder private var statusIcon: some View {
        if !call.isFinished {
            ProgressView().controlSize(.small)
        } else if call.isError {
            Image(systemName: "xmark.circle.fill").foregroundStyle(.red)
        } else {
            Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
        }
    }

    private var inputText: String {
        switch call.name {
        case "bash":
            return call.input["command"]?.stringValue ?? call.input.prettyPrinted
        case "run_python":
            return call.input["code"]?.stringValue ?? call.input.prettyPrinted
        case "write_file":
            let path = call.input["path"]?.stringValue ?? ""
            let content = call.input["content"]?.stringValue ?? ""
            return "\(path)\n\n\(content)"
        default:
            return call.input.prettyPrinted
        }
    }
}

/// Monospaced, selectable, horizontally scrolling text with a copy button.
struct CodeBox: View {
    var label: String?
    let text: String
    var isError = false

    /// Very long outputs are cut for display; the copy button still copies everything.
    private static let displayLimit = 20_000

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                if let label {
                    Text(label)
                        .font(.caption.weight(.medium))
                        .foregroundStyle(isError ? .red : .secondary)
                }
                Spacer()
                Button {
                    UIPasteboard.general.string = text
                } label: {
                    Image(systemName: "doc.on.doc").font(.caption)
                }
                .buttonStyle(.plain)
                .foregroundStyle(.secondary)
                .accessibilityLabel("Copy")
            }
            .padding(.horizontal, 10)
            .padding(.top, 6)
            ScrollView(.horizontal) {
                Text(displayText)
                    .font(.system(.footnote, design: .monospaced))
                    .textSelection(.enabled)
                    .fixedSize(horizontal: true, vertical: false)
                    .padding(10)
            }
        }
        .background(Color(.tertiarySystemBackground), in: RoundedRectangle(cornerRadius: 8))
    }

    private var displayText: String {
        guard text.count > Self.displayLimit else { return text }
        return String(text.prefix(Self.displayLimit)) + "\n… (\(text.count - Self.displayLimit) more characters)"
    }
}
