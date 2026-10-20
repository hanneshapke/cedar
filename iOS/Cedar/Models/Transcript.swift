import Foundation

struct Usage: Hashable, Sendable {
    var inputTokens = 0
    var outputTokens = 0
    /// Summed from the `usage` events that carry one (models with a price).
    var costUSD: Double?

    static let zero = Usage()
}

/// A page, file or search result the agent read, numbered as the model cites it.
struct Source: Hashable, Sendable {
    let n: Int
    let url: String
    let title: String?
}

/// A tool call the agent made, and its result once it arrives.
struct ToolCall: Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let input: JSONValue
    var output: String?
    var isError = false
    var durationMs: Int?
    var sources: [Source] = []
    /// The card the app draws for a finished card tool, parsed once when its result arrives.
    var card: Card?

    var isFinished: Bool { output != nil }

    /// Tools whose result is drawn as a card (map, recipe, route, stocks, products, and a chart from its
    /// data), here and in the web UI.
    static let cardTools: Set<String> = ["show_map", "show_recipe", "plan_trip", "compare_stocks", "search_products", "generate_chart"]

    var isCard: Bool { Self.cardTools.contains(name) }

    /// A one-line description: the command for `bash`, the path for the file tools, a title
    /// for the cards.
    var summary: String {
        switch name {
        case "bash":
            return input["command"]?.stringValue ?? ""
        case "run_python":
            return input["code"]?.stringValue?.split(whereSeparator: \.isNewline).first.map(String.init) ?? ""
        case "read_file", "write_file", "read_document", "generate_chart", "generate_pdf", "download_file":
            return input["path"]?.stringValue ?? input["url"]?.stringValue ?? ""
        case "fetch_url":
            return input["url"]?.stringValue ?? ""
        case "web_search":
            return input["query"]?.stringValue ?? ""
        case "show_map", "show_recipe", "plan_trip", "compare_stocks", "search_products":
            return cardTitle ?? ""
        default:
            return input.prettyPrinted.replacingOccurrences(of: "\n", with: " ")
        }
    }

    /// What a card is about, from its result when it has one, else its input.
    var cardTitle: String? {
        if let output, let result = try? JSONDecoder().decode(JSONValue.self, from: Data(output.utf8)) {
            if let title = result["title"]?.stringValue, !title.isEmpty { return title }
            if name == "compare_stocks", case .array(let symbols)? = result["symbols"] {
                let names = symbols.compactMap { $0["symbol"]?.stringValue }
                if !names.isEmpty { return names.joined(separator: ", ") }
            }
            if name == "plan_trip", let from = result["from"]?["label"]?.stringValue, let to = result["to"]?["label"]?.stringValue {
                return "\(from) → \(to)"
            }
        }
        if let title = input["title"]?.stringValue ?? input["dish"]?.stringValue ?? input["query"]?.stringValue { return title }
        if name == "plan_trip", let from = input["from"]?.stringValue, let to = input["to"]?.stringValue { return "\(from) → \(to)" }
        if case .array(let symbols)? = input["symbols"] {
            return symbols.compactMap(\.stringValue).joined(separator: ", ")
        }
        return nil
    }
}

/// A choice the agent offers for a clarifying question.
struct QuestionOption: Hashable, Sendable {
    let label: String
    let description: String?
}

/// One clarifying question from `ask_user`.
struct Question: Hashable, Sendable {
    let question: String
    let header: String?
    let multiSelect: Bool
    let options: [QuestionOption]
}

/// What an `ask_user` call asked: the run stops after it, and the next message answers.
struct Questions: Hashable, Sendable {
    let questions: [Question]
    let note: String?

    /// Parses the tool's JSON result. `nil` when it holds no question.
    static func parse(_ output: String) -> Questions? {
        guard let value = try? JSONDecoder().decode(JSONValue.self, from: Data(output.utf8)),
              case .array(let items)? = value["questions"]
        else { return nil }
        let questions: [Question] = items.compactMap { item in
            guard let text = item["question"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else { return nil }
            var options: [QuestionOption] = []
            if case .array(let raw)? = item["options"] {
                options = raw.compactMap { o in
                    guard let label = o["label"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines), !label.isEmpty else { return nil }
                    return QuestionOption(label: label, description: o["description"]?.stringValue)
                }
            }
            return Question(question: text, header: item["header"]?.stringValue, multiSelect: item["multi_select"]?.boolValue ?? false, options: options)
        }
        guard !questions.isEmpty else { return nil }
        return Questions(questions: questions, note: value["note"]?.stringValue)
    }
}

/// A tool call waiting for the user's say-so (`approval.required`), and how it was decided.
struct Approval: Identifiable, Hashable, Sendable {
    let id: String
    let callId: String?
    let tool: String
    let input: JSONValue
    let rule: String
    let reason: String
    let scope: String?
    var decision: String?
    var by: String?

    var isPending: Bool { decision == nil }

    /// What "always" would remember: the host, the packages.
    var alwaysLabel: String {
        if let scope, !scope.isEmpty { return "Always for \(scope)" }
        return "Always"
    }
}

struct PlanStep: Hashable, Sendable {
    let id: String
    let title: String
}

/// The steps the agent proposed before starting (`plan`), and whether it waits for them to be
/// agreed.
struct Plan: Hashable, Sendable {
    var steps: [PlanStep]
    let editable: Bool
    var awaiting: Bool
    var by: String?

    static func parseSteps(_ value: JSONValue?) -> [PlanStep] {
        guard case .array(let items)? = value else { return [] }
        return items.enumerated().compactMap { offset, item in
            if let title = item.stringValue { return PlanStep(id: String(offset + 1), title: title) }
            guard let title = item["title"]?.stringValue else { return nil }
            let id = item["id"]?.stringValue ?? item["id"]?.intValue.map { String($0) } ?? String(offset + 1)
            return PlanStep(id: id, title: title)
        }
    }
}

/// What the agent produced during a turn, in order.
enum TranscriptItem: Identifiable, Hashable, Sendable {
    case text(id: Int, String)
    case reasoning(id: Int, text: String, durationMs: Int?)
    case plan(id: Int, Plan)
    case approval(Approval)
    case tool(ToolCall)

    var id: String {
        switch self {
        case .text(let seq, _): return "text-\(seq)"
        case .reasoning(let seq, _, _): return "reasoning-\(seq)"
        case .plan(let seq, _): return "plan-\(seq)"
        case .approval(let approval): return "approval-\(approval.id)"
        case .tool(let call): return "tool-\(call.id)"
        }
    }
}

/// One turn of a conversation: the user's message and the run that answers it, rebuilt from the
/// run's event log.
struct TurnTranscript: Identifiable, Hashable, Sendable {
    var turn: Turn
    var items: [TranscriptItem] = []
    var usage: Usage = .zero
    /// The last event applied; reconnecting resumes after it.
    var lastSeq = 0
    var cancelReason: String?
    /// What `ask_user` asked, once its result arrived.
    var questions: Questions?

    var id: String { turn.id }
    var prompt: String { turn.prompt }
    var isActive: Bool { turn.isActive }

    init(turn: Turn) {
        self.turn = turn
    }

    /// Applies one event. Events at or before `lastSeq` are ignored, so replays are harmless.
    mutating func apply(_ event: RunEvent) {
        guard event.seq > lastSeq else { return }
        lastSeq = event.seq
        let data = event.data
        switch event.type {
        case "run.started":
            turn.status = .running
            usage = .zero
        case "assistant.reasoning":
            if let text = data["text"]?.stringValue, !text.isEmpty {
                items.append(.reasoning(id: event.seq, text: text, durationMs: data["duration_ms"]?.intValue))
            }
        case "assistant.text":
            if let text = data["text"]?.stringValue, !text.isEmpty {
                items.append(.text(id: event.seq, text))
            }
        case "plan":
            let plan = Plan(steps: Plan.parseSteps(data["steps"]), editable: data["editable"]?.boolValue ?? false,
                            awaiting: data["awaiting"]?.boolValue ?? false)
            items.append(.plan(id: event.seq, plan))
        case "plan.updated":
            guard let index = items.lastIndex(where: { if case .plan = $0 { return true } else { return false } }),
                  case .plan(let seq, var plan) = items[index]
            else { return }
            let steps = Plan.parseSteps(data["steps"])
            if !steps.isEmpty { plan.steps = steps }
            plan.awaiting = false
            plan.by = data["by"]?.stringValue
            items[index] = .plan(id: seq, plan)
        case "approval.required":
            let approval = Approval(
                id: data["id"]?.stringValue ?? "seq-\(event.seq)",
                callId: data["call_id"]?.stringValue,
                tool: data["tool"]?.stringValue ?? "tool",
                input: data["input"] ?? .null,
                rule: data["rule"]?.stringValue ?? "",
                reason: data["reason"]?.stringValue ?? "",
                scope: data["scope"]?.stringValue
            )
            items.append(.approval(approval))
        case "approval.resolved":
            guard let id = data["id"]?.stringValue,
                  let index = items.lastIndex(where: { $0.id == "approval-\(id)" }),
                  case .approval(var approval) = items[index]
            else { return }
            approval.decision = data["decision"]?.stringValue ?? "timeout"
            approval.by = data["by"]?.stringValue
            items[index] = .approval(approval)
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
            if call.isCard, !call.isError, let output = call.output {
                call.card = Card.parse(name: call.name, output: output, input: call.input)
            }
            if case .array(let sources)? = data["sources"] {
                call.sources = sources.compactMap { s in
                    guard let n = s["n"]?.intValue, let url = s["url"]?.stringValue else { return nil }
                    return Source(n: n, url: url, title: s["title"]?.stringValue)
                }
            }
            items[index] = .tool(call)
            if call.name == "ask_user", !call.isError, let output = call.output {
                questions = Questions.parse(output)
            }
        case "usage":
            usage.inputTokens += data["input_tokens"]?.intValue ?? 0
            usage.outputTokens += data["output_tokens"]?.intValue ?? 0
            if case .number(let cost)? = data["cost_usd"] {
                usage.costUSD = (usage.costUSD ?? 0) + cost
            }
        case "run.completed":
            turn.status = .completed
            turn.finalText = data["final_text"]?.stringValue
            turn.stopReason = data["stop_reason"]?.stringValue
        case "run.failed":
            turn.status = .failed
            turn.error = data["error"]?.stringValue
        case "run.cancelled":
            turn.status = .cancelled
            cancelReason = data["reason"]?.stringValue
        default:
            break
        }
    }

    /// The agent's answer: the final text, or failing that the last text it wrote.
    var answer: String {
        if let final = turn.finalText, !final.isEmpty { return final }
        for item in items.reversed() {
            if case .text(_, let text) = item { return text }
        }
        return ""
    }

    /// The approval the run is waiting for, if any.
    var pendingApproval: Approval? {
        for item in items.reversed() {
            if case .approval(let approval) = item, approval.isPending { return approval }
        }
        return nil
    }

    /// The plan the run is waiting to have agreed, if any.
    var awaitingPlan: Plan? {
        for item in items.reversed() {
            if case .plan(_, let plan) = item, plan.awaiting { return plan }
        }
        return nil
    }

    /// The images the turn's finished tool calls produced, each once, in order: workspace paths.
    var images: [String] {
        var seen: Set<String> = []
        var paths: [String] = []
        for item in items {
            // A chart drawn as a card isn't shown again as a picture.
            guard case .tool(let call) = item, call.isFinished, !call.isError,
                  !(call.name == "generate_chart" && call.card != nil) else { continue }
            for path in WorkspaceImages.produced(by: call) where seen.insert(path).inserted {
                paths.append(path)
            }
        }
        return Array(paths.prefix(WorkspaceImages.maxShown))
    }
}

/// Which workspace files a tool call produced that are images; the same rule as the web UI's
/// `image-view.js`.
enum WorkspaceImages {
    static let maxShown = 12
    static let extensions: Set<String> = ["png", "jpg", "jpeg", "gif", "webp", "bmp", "avif", "svg"]
    /// Tools whose `path` input is the file they write.
    static let fileTools: Set<String> = ["write_file", "generate_chart", "download_file"]
    static let changedHeading = "\n[files created or modified]\n"

    static func produced(by call: ToolCall) -> [String] {
        var paths: [String] = []
        if fileTools.contains(call.name) {
            if let path = call.input["path"]?.stringValue { paths.append(path) }
        } else if call.name == "bash" || call.name == "run_python", let output = call.output,
                  let range = output.range(of: changedHeading) {
            for line in output[range.upperBound...].split(separator: "\n", omittingEmptySubsequences: false) {
                guard line.hasPrefix("- ") else { break }
                paths.append(String(line.dropFirst(2)))
            }
        }
        return paths.compactMap(workspacePath).filter(isImage)
    }

    /// A path relative to the workspace, or nil if `path` points elsewhere.
    static func workspacePath(_ path: String) -> String? {
        var p = path.trimmingCharacters(in: .whitespacesAndNewlines)
        if p.hasPrefix("/workspace/") {
            p = String(p.dropFirst("/workspace/".count))
        } else if p.hasPrefix("./") {
            p = String(p.dropFirst(2))
        }
        guard !p.isEmpty, !p.hasPrefix("/") else { return nil }
        let parts = p.split(separator: "/", omittingEmptySubsequences: false)
        guard parts.allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." }) else { return nil }
        return p
    }

    static func isImage(_ path: String) -> Bool {
        guard let dot = path.lastIndex(of: ".") else { return false }
        return extensions.contains(path[path.index(after: dot)...].lowercased())
    }
}

/// The text that answers clarifying questions, as the web UI composes it (`clarify-view.js`):
/// each question, then what was chosen and written, so the model sees both.
enum ClarifyAnswer {
    struct Answer: Hashable, Sendable {
        var chosen: [String] = []
        var own = ""
    }

    static func text(for questions: Questions, answers: [Answer]) -> String {
        let lines = questions.questions.enumerated().map { index, q -> String in
            let answer = index < answers.count ? answers[index] : Answer()
            var parts = answer.chosen
            let own = answer.own.trimmingCharacters(in: .whitespacesAndNewlines)
            if !own.isEmpty {
                parts.append(answer.chosen.isEmpty && q.options.isEmpty ? own : "Other: \(own)")
            }
            let text = parts.isEmpty ? "No preference" : parts.joined(separator: ", ")
            return questions.questions.count == 1 ? "\(q.question)\n\(text)" : "\(index + 1). \(q.question)\n\(text)"
        }
        return lines.joined(separator: "\n\n")
    }
}
