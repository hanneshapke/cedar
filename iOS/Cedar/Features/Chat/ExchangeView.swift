import SwiftUI
import UIKit

/// A prompt and everything the agent did to answer it.
struct ExchangeView: View {
    let exchange: Exchange

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            UserBubble(text: exchange.prompt)
            ForEach(exchange.items) { item in
                switch item {
                case .text(_, let text):
                    MarkdownView(text: text)
                case .tool(let call):
                    ToolCallView(call: call)
                }
            }
            if exchange.items.isEmpty, let final = exchange.run.finalText, !final.isEmpty {
                MarkdownView(text: final)
            }
            StatusFooter(exchange: exchange)
        }
    }
}

private struct StatusFooter: View {
    let exchange: Exchange
    @State private var copied = false

    var body: some View {
        switch exchange.run.status {
        case .queued:
            Label { Text("Queued") } icon: { ProgressView().controlSize(.small) }
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .running:
            Label { Text("Working…") } icon: { ProgressView().controlSize(.small) }
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .failed:
            Label(exchange.run.error ?? "The run failed.", systemImage: "exclamationmark.triangle.fill")
                .font(.footnote)
                .foregroundStyle(.red)
                .textSelection(.enabled)
        case .cancelled:
            Label(exchange.cancelReason.map { "Stopped: \($0)" } ?? "Stopped", systemImage: "stop.circle")
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .completed:
            HStack(spacing: 12) {
                Button {
                    UIPasteboard.general.string = exchange.answer
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
                if exchange.usage != .zero {
                    Text("\(exchange.usage.inputTokens.formatted()) in · \(exchange.usage.outputTokens.formatted()) out")
                }
                Text(exchange.run.model.split(separator: "/").last.map(String.init) ?? exchange.run.model)
                    .lineLimit(1)
            }
            .font(.caption)
            .foregroundStyle(.secondary)
        }
    }
}

/// A collapsible tool call: a one-line summary, and the input and output when expanded.
struct ToolCallView: View {
    let call: ToolCall
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
                        .font(.caption.monospaced())
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
                    CodeBox(label: "Input", text: inputText)
                    if let output = call.output {
                        CodeBox(label: call.isError ? "Error" : "Output",
                                text: output.isEmpty ? "(no output)" : output,
                                isError: call.isError)
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
        case "read_file": return "doc.text"
        case "write_file": return "square.and.pencil"
        default: return "wrench.and.screwdriver"
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
