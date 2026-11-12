import Foundation

// The shapes cedar-chat's `/api` returns (see chat/README.md, "API"). Field names are the
// server's, in snake_case; dates are RFC 3339.

/// `GET /api/config`.
struct ServerConfig: Decodable, Sendable {
    let authRequired: Bool
    let adminEnabled: Bool
    /// The server has `/api/memory`; an older one leaves it out, so Settings hides Memory.
    let memory: Bool
    /// The server has `/api/records`; an older one leaves it out, so Settings hides Records.
    let records: Bool
    /// The server can send a conversation to a training corpus (`CHAT_CONTRIBUTIONS_URL`).
    let contributions: Bool

    enum CodingKeys: String, CodingKey {
        case authRequired = "auth_required"
        case adminEnabled = "admin_enabled"
        case memory, records, contributions
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        authRequired = try c.decode(Bool.self, forKey: .authRequired)
        adminEnabled = try c.decode(Bool.self, forKey: .adminEnabled)
        memory = try c.decodeIfPresent(Bool.self, forKey: .memory) ?? false
        records = try c.decodeIfPresent(Bool.self, forKey: .records) ?? false
        contributions = try c.decodeIfPresent(Bool.self, forKey: .contributions) ?? false
    }
}

// MARK: Contributing a conversation (chat/README.md, "Contributing a conversation")

/// `GET /api/conversations/{id}/contribution`: the conversation as it would be contributed, and
/// why it can't be, if it can't. `bundle` is the document itself (`contrib-schema`), kept as JSON:
/// what Send posts back is exactly this object, outcome and ratings put in.
struct ContributionPreview: Decodable, Sendable {
    let bundle: JSONValue
    /// `scheduled` (an agent's run), `active`, `no_turns`, `private_tools`, `run_gone`, `secrets`, `not_consistent`, `model_not_allowed`.
    let blocked: [String]
    let secrets: [String]
    let problems: [String]
    let canSend: Bool
    let accepted: Bool
    let privateTools: [String]
    /// The chat's id of each turn in the bundle, in order, for the rating buttons.
    let turnIds: [String]

    enum CodingKeys: String, CodingKey {
        case bundle, blocked, secrets, problems, accepted
        case canSend = "can_send"
        case privateTools = "private_tools"
        case turnIds = "turn_ids"
    }

    /// One turn of the bundle, as the review shows it.
    struct TurnSummary: Identifiable, Sendable {
        var id: Int { seq }
        let seq: Int
        let prompt: String
        let status: String
        let day: String
        let rating: Int?
        /// The agent's own text, in order.
        let agentText: [String]
        /// The tools it called, in order, with their inputs pretty-printed.
        let toolCalls: [(name: String, input: String)]
    }

    var modelName: String { bundle["model"]?["name"]?.stringValue ?? "" }

    /// The stand-ins the redaction wrote, by kind: values a reader may take for real and shouldn't.
    var standIns: [String: [String]] {
        guard case .object(let kinds)? = bundle["redactions"] else { return [:] }
        var out: [String: [String]] = [:]
        for (kind, list) in kinds {
            let values: [JSONValue] = list.arrayValue ?? []
            out[kind] = values.compactMap { $0.stringValue }
        }
        return out
    }

    var standInCount: Int {
        var n = 0
        for (_, values) in standIns { n += values.count }
        return n
    }

    /// What was left out: `{attachments, memory, fetched_bodies, reasoning}`.
    var dropped: JSONValue? { bundle["dropped"] }

    func turns() -> [TurnSummary] {
        let turnValues: [JSONValue] = bundle["turns"]?.arrayValue ?? []
        var out: [TurnSummary] = []
        for t in turnValues {
            let messages: [JSONValue] = t["messages"]?.arrayValue ?? []
            var agentText: [String] = []
            var toolCalls: [(name: String, input: String)] = []
            for (index, m) in messages.enumerated() {
                let role: String = m["role"]?.stringValue ?? ""
                let blocks: [JSONValue] = m["content"]?.arrayValue ?? []
                for b in blocks {
                    let type: String = b["type"]?.stringValue ?? ""
                    if type == "text", role == "assistant", index > 0 {
                        if let text = b["text"]?.stringValue { agentText.append(text) }
                    } else if type == "tool_use" {
                        let name: String = b["name"]?.stringValue ?? "?"
                        let input: String = b["input"]?.prettyPrinted ?? ""
                        toolCalls.append((name: name, input: input))
                    }
                }
            }
            let summary = TurnSummary(
                seq: t["seq"]?.intValue ?? 0,
                prompt: t["prompt"]?.stringValue ?? "",
                status: t["status"]?.stringValue ?? "",
                day: t["day"]?.stringValue ?? "",
                rating: t["rating"]?.intValue,
                agentText: agentText,
                toolCalls: toolCalls
            )
            out.append(summary)
        }
        return out
    }

    /// The bundle with the person's outcome and the turns' ratings applied, as Send posts it.
    func bundle(outcome: String?, notes: String?, ratings: [Int: Int?]) -> JSONValue {
        guard case .object(var map) = bundle else { return bundle }
        if let outcome {
            var o: [String: JSONValue] = ["result": .string(outcome)]
            let trimmed: String = notes?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            if !trimmed.isEmpty { o["notes"] = .string(trimmed) }
            map["outcome"] = .object(o)
        } else {
            map.removeValue(forKey: "outcome")
        }
        if !ratings.isEmpty, let turns = map["turns"]?.arrayValue {
            var updated: [JSONValue] = []
            for t in turns {
                guard case .object(var tm) = t, let seq = t["seq"]?.intValue, let chosen = ratings[seq] else {
                    updated.append(t)
                    continue
                }
                if let rating = chosen {
                    tm["rating"] = .number(Double(rating))
                } else {
                    tm.removeValue(forKey: "rating")
                }
                updated.append(.object(tm))
            }
            map["turns"] = .array(updated)
        }
        return .object(map)
    }
}

/// `POST /api/conversations/{id}/contribution`.
struct SendContribution: Encodable, Sendable {
    let bundle: JSONValue
    let outcome: JSONValue?
}

/// A contribution's receipt (`GET /api/contributions`).
struct Contribution: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let conversationId: String?
    let title: String?
    let license: String
    let submittedAt: Date
    let withdrawnAt: Date?
    /// What the corpus last said: `quarantined`, `accepted`, `held`, `rejected`, `released`, `unknown`.
    let status: String?
    let releasedIn: [String]
    let checkedAt: Date?

    enum CodingKeys: String, CodingKey {
        case id, title, license, status
        case conversationId = "conversation_id"
        case submittedAt = "submitted_at"
        case withdrawnAt = "withdrawn_at"
        case releasedIn = "released_in"
        case checkedAt = "checked_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        conversationId = try c.decodeIfPresent(String.self, forKey: .conversationId)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        license = try c.decodeIfPresent(String.self, forKey: .license) ?? ""
        submittedAt = try c.decode(Date.self, forKey: .submittedAt)
        withdrawnAt = try c.decodeIfPresent(Date.self, forKey: .withdrawnAt)
        status = try c.decodeIfPresent(String.self, forKey: .status)
        releasedIn = try c.decodeIfPresent([String].self, forKey: .releasedIn) ?? []
        checkedAt = try c.decodeIfPresent(Date.self, forKey: .checkedAt)
    }
}

struct ContributionList: Decodable, Sendable {
    let contributions: [Contribution]
}

/// `GET /api/settings/contributions`: the one-time choices, and whether this server can send at all.
struct ContributionSettings: Decodable, Sendable {
    let license: String
    let pseudonym: String?
    let includeReasoning: Bool
    let includeFetched: Bool
    /// Set once the person read the page and turned contributing on.
    let acceptedAt: Date?
    let available: Bool
    let host: String?
    let licenses: [String]

    var enabled: Bool { acceptedAt != nil }

    enum CodingKeys: String, CodingKey {
        case license, pseudonym, available, host, licenses
        case includeReasoning = "include_reasoning"
        case includeFetched = "include_fetched"
        case acceptedAt = "accepted_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        license = try c.decodeIfPresent(String.self, forKey: .license) ?? "cc0"
        pseudonym = try c.decodeIfPresent(String.self, forKey: .pseudonym)
        includeReasoning = try c.decodeIfPresent(Bool.self, forKey: .includeReasoning) ?? false
        includeFetched = try c.decodeIfPresent(Bool.self, forKey: .includeFetched) ?? false
        acceptedAt = try c.decodeIfPresent(Date.self, forKey: .acceptedAt)
        available = try c.decodeIfPresent(Bool.self, forKey: .available) ?? false
        host = try c.decodeIfPresent(String.self, forKey: .host)
        licenses = try c.decodeIfPresent([String].self, forKey: .licenses) ?? []
    }
}

/// `PUT /api/settings/contributions`: any of these; `pseudonym` is `"new"` (make one) or `""` (drop it).
struct ContributionSettingsPatch: Encodable, Sendable {
    var license: String?
    var pseudonym: String?
    var includeReasoning: Bool?
    var includeFetched: Bool?
    var enabled: Bool?

    enum CodingKeys: String, CodingKey {
        case license, pseudonym, enabled
        case includeReasoning = "include_reasoning"
        case includeFetched = "include_fetched"
    }
}

/// One thing cedar remembers about the user across conversations (`GET /api/memory`): a fact,
/// preference, instruction or temporary fact they stated, an episode (what a conversation was
/// about) or a procedure (how a task went well). See "Memory" in the top-level README.
struct MemoryItem: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let text: String
    /// `fact`, `preference`, `instruction`, `temporary`, `episode` or `procedure`; `fact` on a
    /// cedar from before kinds.
    let kind: String
    /// The user's own words it was taken from, when a conversation stated it.
    let support: String?
    /// Always in cedar's prompt; never archived.
    let pinned: Bool
    /// `user`, `agent`, `consolidation` or `api`.
    let source: String?
    let recallCount: Int
    /// The conversation it was saved in, while that still exists on this server, and its title.
    let conversationId: String?
    let title: String?
    let createdAt: Date
    let validFrom: Date?
    let validUntil: Date?
    /// Out of cedar's prompt, kept until deleted; `supersededBy` names the newer version.
    let archivedAt: Date?
    let supersededBy: String?

    var isActive: Bool { archivedAt == nil && supersededBy == nil }
    var isAboutUser: Bool { kind != "episode" && kind != "procedure" }

    enum CodingKeys: String, CodingKey {
        case id, text, kind, support, pinned, source, title
        case recallCount = "recall_count"
        case conversationId = "conversation_id"
        case createdAt = "created_at"
        case validFrom = "valid_from"
        case validUntil = "valid_until"
        case archivedAt = "archived_at"
        case supersededBy = "superseded_by"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        text = try c.decode(String.self, forKey: .text)
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? "fact"
        support = try c.decodeIfPresent(String.self, forKey: .support)
        pinned = try c.decodeIfPresent(Bool.self, forKey: .pinned) ?? false
        source = try c.decodeIfPresent(String.self, forKey: .source)
        recallCount = try c.decodeIfPresent(Int.self, forKey: .recallCount) ?? 0
        conversationId = try c.decodeIfPresent(String.self, forKey: .conversationId)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        createdAt = try c.decode(Date.self, forKey: .createdAt)
        validFrom = try c.decodeIfPresent(Date.self, forKey: .validFrom)
        validUntil = try c.decodeIfPresent(Date.self, forKey: .validUntil)
        archivedAt = try c.decodeIfPresent(Date.self, forKey: .archivedAt)
        supersededBy = try c.decodeIfPresent(String.self, forKey: .supersededBy)
    }
}

/// The short summary of the user cedar reads before every reply, written by its background
/// pass unless the user wrote it (`userEdited`; then it stays as written).
struct MemoryProfile: Decodable, Hashable, Sendable {
    let text: String
    let userEdited: Bool
    let updatedAt: Date

    enum CodingKeys: String, CodingKey {
        case text
        case userEdited = "user_edited"
        case updatedAt = "updated_at"
    }
}

/// Which memories Settings → Memory lists: the rows about the user, past conversations, ways of
/// working, or the archive (superseded, expired and forgotten rows).
enum MemoryFilter: String, CaseIterable, Identifiable, Sendable {
    case about, conversations, procedures, archive

    var id: String { rawValue }

    /// The `kind` query, `nil` for the server's default (the rows about the user).
    var kind: String? {
        switch self {
        case .about: return nil
        case .conversations: return "episode"
        case .procedures: return "procedure"
        case .archive: return nil
        }
    }

    var archived: Bool { self == .archive }

    /// Every kind of active memory, for counting them all whatever the picker shows.
    static let allKinds = "fact,preference,instruction,temporary,episode,procedure"
}

/// What cedar remembers, oldest first, with the profile; `supported` is false on a cedar from
/// before memory.
struct MemoryList: Decodable, Sendable {
    struct Limits: Decodable, Sendable {
        let maxItems: Int
        let maxChars: Int
        let maxProfileChars: Int?

        enum CodingKeys: String, CodingKey {
            case maxItems = "max_items"
            case maxChars = "max_chars"
            case maxProfileChars = "max_profile_chars"
        }
    }

    let supported: Bool
    let items: [MemoryItem]
    let profile: MemoryProfile?
    let limits: Limits?

    static let unsupported = MemoryList(supported: false, items: [], profile: nil, limits: nil)
}

/// The answer to `GET` and `PUT /api/memory/profile`.
struct MemoryProfileResponse: Decodable, Sendable {
    let profile: MemoryProfile?
}

/// A change to one memory (`PATCH /api/memory/{id}`); fields left `nil` stay as they are.
struct MemoryChange: Encodable, Sendable {
    var pinned: Bool?
    var archived: Bool?
}

/// One set of records cedar keeps for the user across conversations (`GET /api/records`): a
/// running log, weekly measurements (see the top-level README, "Records").
struct RecordSet: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let name: String
    let description: String
    /// The fields that identify a row; empty when every save appends.
    let keyFields: [String]
    let healthData: Bool
    let count: Int
    let columns: [String]
    let updatedAt: Date

    enum CodingKeys: String, CodingKey {
        case id, name, description, count, columns
        case keyFields = "key_fields"
        case healthData = "health_data"
        case updatedAt = "updated_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        name = try c.decode(String.self, forKey: .name)
        description = try c.decodeIfPresent(String.self, forKey: .description) ?? ""
        keyFields = try c.decodeIfPresent([String].self, forKey: .keyFields) ?? []
        healthData = try c.decodeIfPresent(Bool.self, forKey: .healthData) ?? false
        count = try c.decodeIfPresent(Int.self, forKey: .count) ?? 0
        columns = try c.decodeIfPresent([String].self, forKey: .columns) ?? []
        updatedAt = try c.decode(Date.self, forKey: .updatedAt)
    }
}

/// The sets cedar keeps, oldest first; `supported` is false on a cedar from before records.
struct RecordsList: Decodable, Sendable {
    struct Limits: Decodable, Sendable {
        let maxSets: Int
        let maxItems: Int

        enum CodingKeys: String, CodingKey {
            case maxSets = "max_sets"
            case maxItems = "max_items"
        }
    }

    let supported: Bool
    let sets: [RecordSet]
    let limits: Limits?

    static let unsupported = RecordsList(supported: false, sets: [], limits: nil)
}

/// One row of a set (`GET /api/records/{name}`), with the conversation whose run saved it last,
/// while that still exists on this server.
struct RecordRow: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let key: String
    let data: [String: JSONValue]
    let runId: String?
    let conversationId: String?
    let title: String?
    let updatedAt: Date

    enum CodingKeys: String, CodingKey {
        case id, key, data, title
        case runId = "run_id"
        case conversationId = "conversation_id"
        case updatedAt = "updated_at"
    }
}

/// A set with all its rows, oldest first.
struct RecordSetDetail: Decodable, Sendable {
    let set: RecordSet
    let records: [RecordRow]
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
    /// The agent whose run this is (the server's name for it is from before agents had other
    /// triggers); `nil` for a chat.
    var scheduleId: String?
    var tools: [String]?
    var unread: Bool
    var needsAnswer: Bool
    /// When it was starred, on any device; `nil` when it isn't.
    var starredAt: Date?
    /// The title of the agent a run belongs to; only the Starred list and a search send it.
    var scheduleTitle: String?
    /// The passage of a message a search matched; only search results carry it.
    var snippet: String?
    var createdAt: Date
    var updatedAt: Date

    enum CodingKeys: String, CodingKey {
        case id, title, model, reasoning, tools, unread, snippet
        case scheduleId = "schedule_id"
        case needsAnswer = "needs_answer"
        case starredAt = "starred_at"
        case scheduleTitle = "schedule_title"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }

    /// The title cedar-chat gave it (the first message), or a placeholder.
    var displayTitle: String {
        let t = (title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return t.isEmpty ? L10n.string("New chat") : t
    }
}

extension Conversation {
    var isStarred: Bool { starredAt != nil }
}

struct ConversationList: Decodable, Sendable {
    let conversations: [Conversation]
}

struct ConversationDetail: Decodable, Sendable {
    let conversation: Conversation
    let turns: [Turn]
    /// The agent whose run this conversation is; `nil` for a chat, and on a server from before
    /// agents (which still sends `conversation.scheduleId`).
    let agent: AgentRef?
    /// What was decided about the follow-ups its runs suggested; a suggestion that isn't here is
    /// still open. Empty on a server from before follow-ups.
    let followups: [FollowUpDecision]

    enum CodingKeys: String, CodingKey {
        case conversation, turns, agent, followups
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        conversation = try c.decode(Conversation.self, forKey: .conversation)
        turns = try c.decode([Turn].self, forKey: .turns)
        agent = try c.decodeIfPresent(AgentRef.self, forKey: .agent)
        followups = try c.decodeIfPresent([FollowUpDecision].self, forKey: .followups) ?? []
    }
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
