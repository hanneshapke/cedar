import Foundation

// Handing work over from the agent to the person (chat/README.md, "The outbox" and "Feedback").
// Both follow the rule the follow-up suggestions set: the model proposes, the app shows a card,
// and nothing happens until the person presses a button.

/// Something the model reported it could not do, with `report_limitation`. The tool records
/// nothing; the person sends it as feedback, which stays on their server, or dismisses it
/// (`PUT /api/turns/{turn_id}/limitations/{call_id}`).
struct LimitationReport: Hashable, Sendable {
    static let tool = "report_limitation"
    static let categories = ["missing_tool", "not_connected", "blocked", "limit", "other"]

    let summary: String
    /// One of `categories`.
    let category: String
    /// What would make it possible, when the model said.
    let needed: String?

    /// Parses the tool's JSON result, `{"limitation": {"summary", "category", "needed"?}, "note"}`.
    /// `nil` when it holds no report with a summary.
    static func parse(_ output: String) -> LimitationReport? {
        guard let value = try? JSONDecoder().decode(JSONValue.self, from: Data(output.utf8)),
              let report = value["limitation"],
              let summary = report["summary"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines), !summary.isEmpty
        else { return nil }
        let category = report["category"]?.stringValue.flatMap { categories.contains($0) ? $0 : nil } ?? "other"
        let needed = report["needed"]?.stringValue?.trimmingCharacters(in: .whitespacesAndNewlines)
        return LimitationReport(summary: summary, category: category, needed: needed.flatMap { $0.isEmpty ? nil : $0 })
    }
}

/// What a tool call's result says when the call was not made but kept in the outbox: it would
/// have sent or changed something in one of the person's accounts, and nobody was there to
/// approve it (`unattended`) or nobody answered in time (`timeout`). `id` names it in
/// `/api/outbox`.
struct HeldNote: Hashable, Sendable {
    let id: String
    let tool: String
    let reason: String
    let why: String

    var timedOut: Bool { why == "timeout" }

    /// Parses a tool result. `nil` unless it is the note cedar leaves for a held call.
    static func parse(_ output: String) -> HeldNote? {
        // Any tool's result may be one, so look before parsing what may be a long page.
        guard output.drop(while: { $0.isWhitespace }).hasPrefix("{\"held\""),
              let value = try? JSONDecoder().decode(JSONValue.self, from: Data(output.utf8)),
              let held = value["held"],
              let id = held["id"]?.stringValue, !id.isEmpty
        else { return nil }
        return HeldNote(id: id, tool: held["tool"]?.stringValue ?? "", reason: held["reason"]?.stringValue ?? "",
                        why: held["why"]?.stringValue ?? "unattended")
    }
}

/// What the person decided about a report, as `GET /api/conversations/{id}` lists it and
/// deciding answers: `sent`, kept on the server as feedback, or `dismissed`, which keeps nothing
/// of what it said. A report without one is still open; the first decision stands.
struct FeedbackDecision: Decodable, Hashable, Sendable {
    let turnId: String?
    /// The id of the tool call that made the report.
    let callId: String
    /// `sent` or `dismissed`.
    let status: String

    var isSent: Bool { status == "sent" }
    var isDismissed: Bool { status == "dismissed" }
    /// What tells one report from another in a conversation: its turn and its tool call.
    var key: String { Self.key(turnId: turnId ?? "", callId: callId) }

    static func key(turnId: String, callId: String) -> String { "\(turnId)/\(callId)" }

    enum CodingKeys: String, CodingKey {
        case status
        case turnId = "turn_id"
        case callId = "call_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        turnId = try c.decodeIfPresent(String.self, forKey: .turnId)
        callId = try c.decodeIfPresent(String.self, forKey: .callId) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
    }
}

/// The answer to deciding about a report: the decision that stands.
struct FeedbackAnswer: Decodable, Sendable {
    let feedback: FeedbackDecision
}

/// The body of `PUT /api/turns/{turn_id}/limitations/{call_id}`: `send` with the report as it
/// was shown and what the person added, or `dismiss` alone.
struct FeedbackBody: Encodable, Hashable, Sendable {
    let decision: String
    var summary: String?
    var category: String?
    var needed: String?
    var comment: String?

    static let dismiss = FeedbackBody(decision: "dismiss")

    static func send(_ report: LimitationReport, comment: String = "") -> FeedbackBody {
        let note = comment.trimmingCharacters(in: .whitespacesAndNewlines)
        return FeedbackBody(decision: "send", summary: report.summary, category: report.category, needed: report.needed,
                            comment: note.isEmpty ? nil : note)
    }
}

/// A call that waits in the outbox, or what became of one (`/api/outbox`): the tool and its
/// input exactly as the model wrote them, a line on what it would do, and where it came from
/// when the run is one of this chat's.
struct OutboxItem: Decodable, Identifiable, Hashable, Sendable {
    /// The agent whose run wanted to make the call.
    struct Origin: Decodable, Hashable, Sendable {
        let id: String
        let title: String
    }

    let id: String
    let tool: String
    let input: JSONValue
    let reason: String
    /// `unattended` or `timeout`.
    let why: String
    /// `pending`, `sending`, `sent`, `failed` (it can be approved again) or `discarded`.
    let status: String
    /// Whether the person changed the input before it went out.
    let edited: Bool
    /// What the tool answered, or why it failed.
    let output: String?
    let createdAt: Date?
    let decidedAt: Date?
    let conversationId: String?
    let title: String?
    let agent: Origin?

    /// Still to be decided: waiting, or tried and refused by the service.
    var isOpen: Bool { status == "pending" || status == "failed" }
    var isFailed: Bool { status == "failed" }
    var isSent: Bool { status == "sent" }
    var isDiscarded: Bool { status == "discarded" }
    var timedOut: Bool { why == "timeout" }

    /// Who wanted it: the agent, else the conversation's title.
    var origin: String? {
        if let name = agent?.title, !name.isEmpty { return name }
        if let title, !title.isEmpty { return title }
        return nil
    }

    /// The input as rows to show, by name: text as it is, anything else as JSON.
    var fields: [Field] {
        guard case .object(let object) = input else { return [] }
        return object.keys.sorted().map { name in
            let value = object[name] ?? .null
            return Field(name: name, value: value.stringValue ?? value.compact)
        }
    }

    struct Field: Identifiable, Hashable, Sendable {
        let name: String
        let value: String
        var id: String { name }
    }

    enum CodingKeys: String, CodingKey {
        case id, tool, input, reason, why, status, edited, output, title, agent
        case createdAt = "created_at"
        case decidedAt = "decided_at"
        case conversationId = "conversation_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        tool = try c.decodeIfPresent(String.self, forKey: .tool) ?? ""
        input = try c.decodeIfPresent(JSONValue.self, forKey: .input) ?? .object([:])
        reason = try c.decodeIfPresent(String.self, forKey: .reason) ?? ""
        why = try c.decodeIfPresent(String.self, forKey: .why) ?? ""
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        edited = try c.decodeIfPresent(Bool.self, forKey: .edited) ?? false
        output = try c.decodeIfPresent(String.self, forKey: .output)
        createdAt = try c.decodeIfPresent(Date.self, forKey: .createdAt)
        decidedAt = try c.decodeIfPresent(Date.self, forKey: .decidedAt)
        conversationId = try c.decodeIfPresent(String.self, forKey: .conversationId)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        agent = try c.decodeIfPresent(Origin.self, forKey: .agent)
    }
}

/// `GET /api/outbox`: what waits for the person's approval, newest first.
struct Outbox: Decodable, Sendable {
    let items: [OutboxItem]

    enum CodingKeys: String, CodingKey { case items }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        items = try c.decodeIfPresent([OutboxItem].self, forKey: .items) ?? []
    }
}

/// The body of `POST /api/outbox/{id}`.
struct OutboxDecisionBody: Encodable, Hashable, Sendable {
    let decision: String

    static let approve = OutboxDecisionBody(decision: "approve")
    static let discard = OutboxDecisionBody(decision: "discard")
}

extension JSONValue {
    /// One line of JSON, for a value that isn't text.
    var compact: String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard let data = try? encoder.encode(self), let text = String(data: data, encoding: .utf8) else { return "" }
        return text
    }
}
