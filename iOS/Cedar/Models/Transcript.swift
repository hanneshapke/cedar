import Foundation

/// A tool call the agent made, and its result once it arrives.
struct ToolCall: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let input: JSONValue
    var output: String?
    var isError = false
    var durationMs: Int?

    var isFinished: Bool { output != nil }

    /// A one-line description: the command for `bash`, the path for the file tools.
    var summary: String {
        switch name {
        case "bash":
            return input["command"]?.stringValue ?? ""
        case "read_file", "write_file":
            return input["path"]?.stringValue ?? ""
        default:
            return input.prettyPrinted.replacingOccurrences(of: "\n", with: " ")
        }
    }
}

/// What the agent produced during a run, in order.
enum TranscriptItem: Identifiable, Hashable, Sendable {
    case text(id: Int, String)
    case tool(ToolCall)

    var id: String {
        switch self {
        case .text(let seq, _): return "text-\(seq)"
        case .tool(let call): return "tool-\(call.id)"
        }
    }
}

/// One exchange of a conversation: the user's prompt and the run that answers it, rebuilt from the
/// run's event log.
struct Exchange: Identifiable, Hashable, Sendable {
    var run: Run
    var items: [TranscriptItem] = []
    var usage: Usage = .zero
    /// The last event applied; reconnecting resumes after it.
    var lastSeq = 0
    var cancelReason: String?

    var id: String { run.id }
    var prompt: String { run.prompt }
    var isActive: Bool { !run.status.isTerminal }

    init(run: Run) {
        self.run = run
        self.usage = run.usage
    }

    /// Applies one event. Events at or before `lastSeq` are ignored, so replays are harmless.
    mutating func apply(_ event: RunEvent) {
        guard event.seq > lastSeq else { return }
        lastSeq = event.seq
        let data = event.data
        switch event.type {
        case "run.started":
            run.status = .running
            usage = .zero
        case "assistant.text":
            if let text = data["text"]?.stringValue, !text.isEmpty {
                items.append(.text(id: event.seq, text))
            }
        case "tool.call":
            let call = ToolCall(
                id: data["id"]?.stringValue ?? "seq-\(event.seq)",
                name: data["name"]?.stringValue ?? "tool",
                input: data["input"] ?? .null
            )
            items.append(.tool(call))
        case "tool.result":
            guard let id = data["id"]?.stringValue,
                  let index = items.lastIndex(where: { $0.id == "tool-\(id)" }),
                  case .tool(var call) = items[index]
            else { return }
            call.output = data["output"]?.stringValue ?? ""
            call.isError = data["is_error"]?.boolValue ?? false
            call.durationMs = data["duration_ms"]?.intValue
            items[index] = .tool(call)
        case "usage":
            usage.inputTokens += data["input_tokens"]?.intValue ?? 0
            usage.outputTokens += data["output_tokens"]?.intValue ?? 0
        case "run.completed":
            run.status = .completed
            run.finalText = data["final_text"]?.stringValue
            run.stopReason = data["stop_reason"]?.stringValue
        case "run.failed":
            run.status = .failed
            run.error = data["error"]?.stringValue
        case "run.cancelled":
            run.status = .cancelled
            cancelReason = data["reason"]?.stringValue
        default:
            break
        }
    }

    /// The agent's answer: the final text, or failing that the last text it wrote.
    var answer: String {
        if let final = run.finalText, !final.isEmpty { return final }
        for item in items.reversed() {
            if case .text(_, let text) = item { return text }
        }
        return ""
    }
}
