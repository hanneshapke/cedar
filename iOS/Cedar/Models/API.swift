import Foundation

// The shapes cedar-chat's `/api` returns (see chat/README.md, "API"). Field names are the
// server's, in snake_case; dates are RFC 3339.

/// `GET /api/config`.
struct ServerConfig: Decodable, Sendable {
    let authRequired: Bool
    let adminEnabled: Bool

    enum CodingKeys: String, CodingKey {
        case authRequired = "auth_required"
        case adminEnabled = "admin_enabled"
    }
}

/// One of the models cedar has registered (`GET /api/models`).
struct ModelInfo: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let label: String
    let model: String
    let reasoningLevels: [String]
    let defaultReasoning: String?
    let multimodal: Bool
    let isDefault: Bool

    enum CodingKeys: String, CodingKey {
        case id, label, model, multimodal
        case reasoningLevels = "reasoning_levels"
        case defaultReasoning = "default_reasoning"
        case isDefault = "is_default"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        model = try c.decodeIfPresent(String.self, forKey: .model) ?? id
        label = try c.decodeIfPresent(String.self, forKey: .label) ?? model
        reasoningLevels = try c.decodeIfPresent([String].self, forKey: .reasoningLevels) ?? []
        defaultReasoning = try c.decodeIfPresent(String.self, forKey: .defaultReasoning)
        multimodal = try c.decodeIfPresent(Bool.self, forKey: .multimodal) ?? false
        isDefault = try c.decodeIfPresent(Bool.self, forKey: .isDefault) ?? false
    }

    init(id: String, label: String, model: String, reasoningLevels: [String] = [], defaultReasoning: String? = nil, multimodal: Bool = false, isDefault: Bool = false) {
        self.id = id
        self.label = label
        self.model = model
        self.reasoningLevels = reasoningLevels
        self.defaultReasoning = defaultReasoning
        self.multimodal = multimodal
        self.isDefault = isDefault
    }
}

struct ModelList: Decodable, Sendable {
    let models: [ModelInfo]
}

/// A conversation as cedar-chat keeps it: its title, the model it last used, and whether a
/// finished response is waiting to be read or answered.
struct Conversation: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    var title: String?
    var model: String?
    var reasoning: String?
    var scheduleId: String?
    var tools: [String]?
    var unread: Bool
    var needsAnswer: Bool
    var createdAt: Date
    var updatedAt: Date

    enum CodingKeys: String, CodingKey {
        case id, title, model, reasoning, tools, unread
        case scheduleId = "schedule_id"
        case needsAnswer = "needs_answer"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }

    /// The title cedar-chat gave it (the first message), or a placeholder.
    var displayTitle: String {
        let t = (title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? "New chat" : t
    }
}

struct ConversationList: Decodable, Sendable {
    let conversations: [Conversation]
}

struct ConversationDetail: Decodable, Sendable {
    let conversation: Conversation
    let turns: [Turn]
}

enum TurnStatus: String, Codable, Sendable {
    case queued, running, completed, failed, cancelled

    var isTerminal: Bool { self != .queued && self != .running }
}

/// A file sent with a message, as the server reports it back.
struct AttachmentInfo: Decodable, Hashable, Sendable {
    let name: String
    let mediaType: String
    let size: Int

    enum CodingKeys: String, CodingKey {
        case name, size
        case mediaType = "media_type"
    }
}

/// One message and the cedar run that answers it.
struct Turn: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let conversationId: String
    let seq: Int
    let runId: String
    let prompt: String
    var status: TurnStatus
    var finalText: String?
    var error: String?
    var stopReason: String?
    var finishedAt: Date?
    var seen: Bool
    var attachments: [AttachmentInfo]
    let createdAt: Date

    enum CodingKeys: String, CodingKey {
        case id, seq, prompt, status, error, seen, attachments
        case conversationId = "conversation_id"
        case runId = "run_id"
        case finalText = "final_text"
        case stopReason = "stop_reason"
        case finishedAt = "finished_at"
        case createdAt = "created_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        conversationId = try c.decode(String.self, forKey: .conversationId)
        seq = try c.decodeIfPresent(Int.self, forKey: .seq) ?? 0
        runId = try c.decode(String.self, forKey: .runId)
        prompt = try c.decodeIfPresent(String.self, forKey: .prompt) ?? ""
        status = try c.decode(TurnStatus.self, forKey: .status)
        finalText = try c.decodeIfPresent(String.self, forKey: .finalText)
        error = try c.decodeIfPresent(String.self, forKey: .error)
        stopReason = try c.decodeIfPresent(String.self, forKey: .stopReason)
        finishedAt = try c.decodeIfPresent(Date.self, forKey: .finishedAt)
        seen = try c.decodeIfPresent(Bool.self, forKey: .seen) ?? true
        attachments = try c.decodeIfPresent([AttachmentInfo].self, forKey: .attachments) ?? []
        createdAt = try c.decodeIfPresent(Date.self, forKey: .createdAt) ?? Date()
    }

    init(id: String, conversationId: String, seq: Int = 1, runId: String, prompt: String, status: TurnStatus,
         finalText: String? = nil, error: String? = nil, stopReason: String? = nil, finishedAt: Date? = nil,
         seen: Bool = true, attachments: [AttachmentInfo] = [], createdAt: Date = Date()) {
        self.id = id
        self.conversationId = conversationId
        self.seq = seq
        self.runId = runId
        self.prompt = prompt
        self.status = status
        self.finalText = finalText
        self.error = error
        self.stopReason = stopReason
        self.finishedAt = finishedAt
        self.seen = seen
        self.attachments = attachments
        self.createdAt = createdAt
    }

    var isActive: Bool { !status.isTerminal }
    /// The agent stopped to ask something (`ask_user`); the next message answers it.
    var needsAnswer: Bool { status == .completed && stopReason == "ask_user" }
}

/// A finished response nobody has looked at (`GET /api/inbox`).
struct InboxItem: Decodable, Identifiable, Hashable, Sendable {
    let turnId: String
    let conversationId: String
    let title: String?
    let scheduleId: String?
    let status: String
    let stopReason: String?
    let needsAnswer: Bool
    let finishedAt: Date?

    var id: String { turnId }

    enum CodingKeys: String, CodingKey {
        case title, status
        case turnId = "turn_id"
        case conversationId = "conversation_id"
        case scheduleId = "schedule_id"
        case stopReason = "stop_reason"
        case needsAnswer = "needs_answer"
        case finishedAt = "finished_at"
    }
}

struct Inbox: Decodable, Sendable {
    let items: [InboxItem]
    let unread: Int
}

/// A scheduled agent (`GET /api/schedules`): a prompt the server starts as a fresh run whenever
/// it is due, each run in a conversation of its own that the conversation list leaves out.
struct Schedule: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    var title: String
    var prompt: String
    var model: String?
    var reasoning: String?
    /// `daily`, `weekdays` or `weekly`.
    var frequency: String
    /// 0–6, Monday–Sunday; weekly only.
    var weekday: Int?
    /// `HH:MM` in `timezone`.
    var timeOfDay: String
    var timezone: String
    var enabled: Bool
    var nextRunAt: Date
    var lastRunAt: Date?
    /// Only in the list; a schedule as `PATCH` returns it has none.
    var lastRun: ScheduleRun?

    enum CodingKeys: String, CodingKey {
        case id, title, prompt, model, reasoning, frequency, weekday, timezone, enabled
        case timeOfDay = "time_of_day"
        case nextRunAt = "next_run_at"
        case lastRunAt = "last_run_at"
        case lastRun = "last_run"
    }

    /// "Weekdays at 07:30", with the schedule's time zone when it isn't this device's.
    var cadence: String {
        let when: String
        switch frequency {
        case "daily": when = "Daily"
        case "weekdays": when = "Weekdays"
        case "weekly":
            let names = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays"]
            when = weekday.flatMap { names.indices.contains($0) ? names[$0] : nil } ?? "Weekly"
        default: when = frequency.capitalized
        }
        let zone = timezone == TimeZone.current.identifier ? "" : " (\(timezone))"
        return "\(when) at \(timeOfDay)\(zone)"
    }
}

/// One time a schedule fired, with the turn that carries its run, or the `error` that kept it
/// from starting.
struct ScheduleRun: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let scheduleId: String
    let conversationId: String?
    let turnId: String?
    let dueAt: Date
    let startedAt: Date
    /// Started with Run now rather than on schedule.
    let manual: Bool
    let error: String?
    let turn: Turn?

    enum CodingKeys: String, CodingKey {
        case id, manual, error, turn
        case scheduleId = "schedule_id"
        case conversationId = "conversation_id"
        case turnId = "turn_id"
        case dueAt = "due_at"
        case startedAt = "started_at"
    }

    enum Outcome: Hashable, Sendable {
        case queued, running, completed, needsAnswer, failed, cancelled, notStarted
    }

    var outcome: Outcome {
        guard let turn else { return .notStarted }
        switch turn.status {
        case .queued: return .queued
        case .running: return .running
        case .completed: return turn.needsAnswer ? .needsAnswer : .completed
        case .failed: return .failed
        case .cancelled: return .cancelled
        }
    }

    var isActive: Bool { turn?.isActive ?? false }
    /// A finished response in it nobody has looked at.
    var unread: Bool { turn.map { !$0.isActive && !$0.seen } ?? false }
}

struct ScheduleList: Decodable, Sendable {
    let schedules: [Schedule]
}

/// `GET /api/schedules/{id}`: the schedule and its runs, newest first.
struct ScheduleDetail: Decodable, Sendable {
    let schedule: Schedule
    let runs: [ScheduleRun]
}

/// A `turn.finished` notice from `GET /api/events`: a response ended somewhere.
struct Notice: Decodable, Hashable, Sendable {
    let turnId: String
    let conversationId: String
    let runId: String?
    let status: String
    let stopReason: String?
    let error: String?
    let needsAnswer: Bool
    let title: String?
    let scheduleId: String?

    enum CodingKeys: String, CodingKey {
        case status, error, title
        case turnId = "turn_id"
        case conversationId = "conversation_id"
        case runId = "run_id"
        case stopReason = "stop_reason"
        case needsAnswer = "needs_answer"
        case scheduleId = "schedule_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        turnId = try c.decode(String.self, forKey: .turnId)
        conversationId = try c.decode(String.self, forKey: .conversationId)
        runId = try c.decodeIfPresent(String.self, forKey: .runId)
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        stopReason = try c.decodeIfPresent(String.self, forKey: .stopReason)
        error = try c.decodeIfPresent(String.self, forKey: .error)
        needsAnswer = try c.decodeIfPresent(Bool.self, forKey: .needsAnswer) ?? false
        title = try c.decodeIfPresent(String.self, forKey: .title)
        scheduleId = try c.decodeIfPresent(String.self, forKey: .scheduleId)
    }
}

/// One entry of a run's event log, as cedar-chat relays it from cedar over Server-Sent Events.
struct RunEvent: Decodable, Hashable, Sendable {
    let seq: Int
    let type: String
    let data: JSONValue

    static let terminalTypes: Set<String> = ["run.completed", "run.failed", "run.cancelled"]

    var isTerminal: Bool { Self.terminalTypes.contains(type) }
}

/// A file under the conversation's `/workspace` (`GET /api/conversations/{id}/files`).
struct WorkspaceFile: Decodable, Identifiable, Hashable, Sendable {
    let path: String
    let size: Int
    let modifiedAt: Date?

    var id: String { path }

    enum CodingKeys: String, CodingKey {
        case path, size
        case modifiedAt = "modified_at"
    }
}

struct WorkspaceFiles: Decodable, Sendable {
    let workspaceId: String?
    let files: [WorkspaceFile]
    let truncated: Bool

    enum CodingKeys: String, CodingKey {
        case files, truncated
        case workspaceId = "workspace_id"
    }
}

extension JSONDecoder {
    /// Decodes the server's payloads, including RFC 3339 timestamps with up to nanosecond precision.
    static let cedar: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let raw = try container.decode(String.self)
            guard let date = CedarDate.parse(raw) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "bad date \(raw)")
            }
            return date
        }
        return decoder
    }()
}

enum CedarDate {
    /// Parses RFC 3339 timestamps. Foundation's ISO 8601 parser only takes millisecond precision,
    /// and chrono writes up to nine fractional digits, so the fraction is trimmed first.
    static func parse(_ raw: String) -> Date? {
        var text = raw
        if let dot = text.firstIndex(of: ".") {
            let fractionStart = text.index(after: dot)
            let fractionEnd = text[fractionStart...].firstIndex { !$0.isNumber } ?? text.endIndex
            var fraction = String(text[fractionStart..<fractionEnd].prefix(3))
            while fraction.count < 3 { fraction += "0" }
            text.replaceSubrange(fractionStart..<fractionEnd, with: fraction)
            return withFraction.date(from: text)
        }
        return withoutFraction.date(from: text)
    }

    private static let withFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private static let withoutFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()
}
