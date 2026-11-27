import Foundation

// Today (`GET /api/timeline`, `GET /api/turns/{id}/outcome`, `POST /api/asks`): one day of what
// Cedar did, is doing and will do, what one ask came to, and asking with a When. Field names are
// the server's, in snake_case. What a server may leave out is optional or has a default here, so
// a newer server's answer still reads.

// MARK: The timeline

/// What a turn's run waits on from the person while it goes: its plan to be agreed to
/// (`kind` is `plan`), or a risky call to be approved (`approval`, keyed `approval:<id>`).
struct TimelineWait: Decodable, Identifiable, Hashable, Sendable {
    let kind: String
    let key: String
    /// An approval's reason, as cedar gave it.
    let reason: String?
    /// A plan's steps, as cedar proposed them; empty for an approval, or on an older server.
    let steps: [PlanStep]

    var id: String { key }
    var isPlan: Bool { kind == "plan" }

    /// The id `POST /api/turns/{id}/approvals/{approval_id}` takes; `nil` for a plan.
    var approvalId: String? {
        let prefix = "approval:"
        guard kind == "approval", key.hasPrefix(prefix) else { return nil }
        let id = String(key.dropFirst(prefix.count))
        return id.isEmpty ? nil : id
    }

    enum CodingKeys: String, CodingKey {
        case kind, key, reason, steps
    }

    /// A step as the server writes it: `{id, title}`.
    private struct Step: Decodable {
        let id: String
        let title: String
    }

    init(kind: String, key: String, reason: String? = nil, steps: [PlanStep] = []) {
        self.kind = kind
        self.key = key
        self.reason = reason
        self.steps = steps
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? ""
        key = try c.decodeIfPresent(String.self, forKey: .key) ?? kind
        reason = try c.decodeIfPresent(String.self, forKey: .reason)
        // Steps of another shape (a newer server's) read as none: the plan then opens its chat.
        let read = try? c.decodeIfPresent([Step].self, forKey: .steps)
        steps = (read ?? []).map { PlanStep(id: $0.id, title: $0.title) }
    }
}

/// Where a response still going is: how many tool calls its run has started, and the latest
/// one's name (the tool's function name, `web_search`).
struct TimelineProgress: Decodable, Hashable, Sendable {
    let steps: Int
    let doing: String?

    enum CodingKeys: String, CodingKey {
        case steps, doing
    }

    init(steps: Int, doing: String?) {
        self.steps = steps
        self.doing = doing
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        steps = try c.decodeIfPresent(Int.self, forKey: .steps) ?? 0
        doing = try c.decodeIfPresent(String.self, forKey: .doing)
    }
}

/// What went wrong with something done from Today, in the server's or the chat's own words.
struct TodayError: LocalizedError, Hashable, Sendable {
    let message: String
    var errorDescription: String? { message }
}

/// Where a turn stands, in the river's words. A state from a newer server reads as `done`.
enum TimelineState: String, Hashable, Sendable {
    /// On the person's go-ahead for a plan, or their OK for a call.
    case waiting
    /// It ended with a question.
    case asking
    case working, done, failed, stopped

    init(server: String) {
        self = TimelineState(rawValue: server) ?? .done
    }

    /// What needs the person.
    var needsYou: Bool { self == .waiting || self == .asking }
}

/// A turn as the river shows it: what was asked, how it went, the agent whose run it is, and
/// what it waits on.
struct TimelineTurn: Decodable, Identifiable, Hashable, Sendable {
    let turnId: String
    let conversationId: String
    let title: String?
    let prompt: String
    let state: TimelineState
    /// The response's outcome in one line; `nil` while it goes, or when it wrote no prose.
    let summary: String?
    let error: String?
    let createdAt: Date
    let finishedAt: Date?
    let needsAnswer: Bool
    let agent: AgentRef?
    /// Open waits; only a turn still going has any.
    let waits: [TimelineWait]
    /// Where it is, once its run has started a tool call; `nil` before that, and when it is over.
    let progress: TimelineProgress?

    var id: String { turnId }

    /// The conversation's title, else what was asked.
    var displayTitle: String {
        let title = (self.title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !title.isEmpty { return title }
        let prompt = self.prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        return prompt.isEmpty ? L10n.string("New chat") : prompt
    }

    /// The line under an earlier row: why it failed, else what it came to.
    var line: String? {
        let text = (state == .failed ? error : summary)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return text.isEmpty ? nil : text
    }

    enum CodingKeys: String, CodingKey {
        case title, prompt, state, summary, error, agent, waits, progress
        case turnId = "turn_id"
        case conversationId = "conversation_id"
        case createdAt = "created_at"
        case finishedAt = "finished_at"
        case needsAnswer = "needs_answer"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        turnId = try c.decode(String.self, forKey: .turnId)
        conversationId = try c.decode(String.self, forKey: .conversationId)
        title = try c.decodeIfPresent(String.self, forKey: .title)
        prompt = try c.decodeIfPresent(String.self, forKey: .prompt) ?? ""
        state = TimelineState(server: try c.decodeIfPresent(String.self, forKey: .state) ?? "")
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        error = try c.decodeIfPresent(String.self, forKey: .error)
        createdAt = try c.decodeIfPresent(Date.self, forKey: .createdAt) ?? Date()
        finishedAt = try c.decodeIfPresent(Date.self, forKey: .finishedAt)
        needsAnswer = try c.decodeIfPresent(Bool.self, forKey: .needsAnswer) ?? false
        agent = try c.decodeIfPresent(AgentRef.self, forKey: .agent)
        waits = try c.decodeIfPresent([TimelineWait].self, forKey: .waits) ?? []
        progress = try c.decodeIfPresent(TimelineProgress.self, forKey: .progress)
    }
}

/// An agent as the river lists it: under ahead with when it runs next, under watching when
/// something outside starts its runs, and among the standing asks.
struct TimelineAgent: Decodable, Identifiable, Hashable, Sendable {
    let id: String
    let title: String
    let enabled: Bool
    let trigger: Trigger
    /// Something outside starts its runs (a value, mail, an event, a call, the person's device).
    let watches: Bool
    let nextRunAt: Date?
    let lastCheckedAt: Date?

    enum CodingKeys: String, CodingKey {
        case id, title, enabled, trigger, watches
        case triggerKind = "trigger_kind"
        case nextRunAt = "next_run_at"
        case lastCheckedAt = "last_checked_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        let kind = try c.decodeIfPresent(String.self, forKey: .triggerKind) ?? ""
        trigger = try c.decodeIfPresent(Trigger.self, forKey: .trigger) ?? .unknown(kind: kind)
        watches = try c.decodeIfPresent(Bool.self, forKey: .watches) ?? Trigger.byEvent(kind: kind)
        nextRunAt = try c.decodeIfPresent(Date.self, forKey: .nextRunAt)
        lastCheckedAt = try c.decodeIfPresent(Date.self, forKey: .lastCheckedAt)
    }
}

/// A call an unattended run held for the person's OK, as the river lists it: the outbox's item,
/// and the turn whose run wanted it when that is one of this chat's.
struct TimelineHeld: Decodable, Identifiable, Hashable, Sendable {
    let item: OutboxItem
    let turnId: String?

    var id: String { item.id }

    enum CodingKeys: String, CodingKey {
        case turnId = "turn_id"
    }

    init(from decoder: Decoder) throws {
        item = try OutboxItem(from: decoder)
        let c = try decoder.container(keyedBy: CodingKeys.self)
        turnId = try c.decodeIfPresent(String.self, forKey: .turnId)
    }
}

/// `GET /api/timeline`: one day as a river. `earlier` is the day's turns, oldest first; `now` is
/// what is going or waits on the person, and `outbox` what unattended runs held for their OK
/// (both today's only, whatever day they are from); `ahead` the agents due next, and `watching`
/// the ones that keep watch. `needsYou` counts the waits, the questions and the outbox.
struct Timeline: Decodable, Sendable {
    /// `YYYY-MM-DD`, in `timezone`.
    let day: String
    let timezone: String
    let today: Bool
    let needsYou: Int
    let earlier: [TimelineTurn]
    let now: [TimelineTurn]
    let outbox: [TimelineHeld]
    let ahead: [TimelineAgent]
    let watching: [TimelineAgent]
    let standing: [TimelineAgent]

    /// Nothing to show at all.
    var isEmpty: Bool { earlier.isEmpty && now.isEmpty && outbox.isEmpty && ahead.isEmpty && watching.isEmpty }

    /// `YYYY-MM-DD` read and written as a calendar date, whatever the device's zone.
    private static let dayFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = TimeZone(identifier: "UTC")
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter
    }()

    /// The day `offset` days from `day` (`YYYY-MM-DD`); `nil` when `day` isn't a date.
    static func day(_ day: String, offsetBy offset: Int) -> String? {
        guard let date = dayFormatter.date(from: day) else { return nil }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? .current
        guard let moved = calendar.date(byAdding: .day, value: offset, to: date) else { return nil }
        return dayFormatter.string(from: moved)
    }

    /// `day` written out for a heading ("Friday, 27 November"), in `locale`; `day` itself when
    /// it isn't a date.
    static func title(of day: String, locale: Locale) -> String {
        guard let date = dayFormatter.date(from: day) else { return day }
        let formatter = DateFormatter()
        formatter.locale = locale
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = TimeZone(identifier: "UTC")
        formatter.setLocalizedDateFormatFromTemplate("EEEEdMMMM")
        return formatter.string(from: date)
    }

    enum CodingKeys: String, CodingKey {
        case day, timezone, today, earlier, now, outbox, ahead, watching, standing
        case needsYou = "needs_you"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        day = try c.decodeIfPresent(String.self, forKey: .day) ?? ""
        timezone = try c.decodeIfPresent(String.self, forKey: .timezone) ?? "UTC"
        today = try c.decodeIfPresent(Bool.self, forKey: .today) ?? true
        needsYou = try c.decodeIfPresent(Int.self, forKey: .needsYou) ?? 0
        earlier = try c.decodeIfPresent([TimelineTurn].self, forKey: .earlier) ?? []
        now = try c.decodeIfPresent([TimelineTurn].self, forKey: .now) ?? []
        outbox = try c.decodeIfPresent([TimelineHeld].self, forKey: .outbox) ?? []
        ahead = try c.decodeIfPresent([TimelineAgent].self, forKey: .ahead) ?? []
        watching = try c.decodeIfPresent([TimelineAgent].self, forKey: .watching) ?? []
        standing = try c.decodeIfPresent([TimelineAgent].self, forKey: .standing) ?? []
    }
}

// MARK: An outcome

/// `GET /api/turns/{id}/outcome`: what one ask came to. The answer, and what the turn left
/// behind: the files it wrote or changed, what it remembered, the record rows it saved, the
/// follow-ups it suggested and what it held for the person's OK, with its steps and the sources
/// it read. A part the server can't give is empty.
struct AskOutcome: Decodable, Sendable {
    struct ConversationRef: Decodable, Hashable, Sendable {
        let id: String
        let title: String?
        let isPrivate: Bool

        enum CodingKeys: String, CodingKey {
            case id, title
            case isPrivate = "private"
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            title = try c.decodeIfPresent(String.self, forKey: .title)
            isPrivate = try c.decodeIfPresent(Bool.self, forKey: .isPrivate) ?? false
        }
    }

    /// One tool call of the run.
    struct Step: Decodable, Identifiable, Hashable, Sendable {
        let id: String
        let name: String
        let isError: Bool

        enum CodingKeys: String, CodingKey {
            case id, name
            case isError = "is_error"
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decodeIfPresent(String.self, forKey: .id) ?? ""
            name = try c.decodeIfPresent(String.self, forKey: .name) ?? ""
            isError = try c.decodeIfPresent(Bool.self, forKey: .isError) ?? false
        }
    }

    /// A page or a file the run read.
    struct Source: Decodable, Hashable, Sendable {
        let url: String?
        let path: String?
        let title: String?

        /// What a row calls it: its title, else its address or path.
        var label: String {
            if let title, !title.isEmpty { return title }
            return url ?? path ?? ""
        }

        /// Where it opens; only http(s) addresses do.
        var link: URL? {
            guard let url, let parsed = URL(string: url), let scheme = parsed.scheme?.lowercased(),
                  scheme == "http" || scheme == "https" else { return nil }
            return parsed
        }
    }

    /// A follow-up the run suggested: the id of the `suggest_follow_up` call and its result.
    struct Suggestion: Decodable, Identifiable, Hashable, Sendable {
        let callId: String
        let output: String

        var id: String { callId }
        /// The suggestion itself; `nil` when the result holds none.
        var followUp: FollowUp? { FollowUp.parse(output) }

        enum CodingKeys: String, CodingKey {
            case output
            case callId = "call_id"
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            callId = try c.decodeIfPresent(String.self, forKey: .callId) ?? ""
            output = try c.decodeIfPresent(String.self, forKey: .output) ?? ""
        }
    }

    /// Something the run remembered about the person.
    struct Memory: Decodable, Identifiable, Hashable, Sendable {
        let id: String
        let text: String
    }

    /// A record row the run saved: the set's name, the row's key and its values.
    struct Record: Decodable, Hashable, Sendable {
        let set: String
        let key: String
        /// The row itself: an object of its fields.
        let data: JSONValue?

        /// "trips: lisbon", or the set alone for a row without a key.
        var label: String { key.isEmpty ? set : L10n.labeled(set, key) }

        /// The row's plain values as "field: value", by field name; lists, objects and empty
        /// values are left out, and at most `limit` are given.
        func values(limit: Int = 8) -> [String] {
            guard case .object(let fields)? = data else { return [] }
            var lines: [String] = []
            for name in fields.keys.sorted() {
                guard let value = fields[name], let text = Self.text(value) else { continue }
                lines.append(L10n.labeled(name, text))
                if lines.count == limit { break }
            }
            return lines
        }

        /// A plain value as text: a whole number without its ".0"; `nil` for what isn't plain.
        private static func text(_ value: JSONValue) -> String? {
            switch value {
            case .string(let s):
                let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
                return trimmed.isEmpty ? nil : trimmed
            case .number(let n):
                if n == n.rounded(), abs(n) < 1e15 { return String(Int(n)) }
                return String(n)
            case .bool(let b):
                return b ? L10n.string("Yes") : L10n.string("No")
            case .null, .array, .object:
                return nil
            }
        }

        enum CodingKeys: String, CodingKey {
            case set, key, data
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            set = try c.decodeIfPresent(String.self, forKey: .set) ?? ""
            // A key is text, but a set keyed by a number says it as one.
            if let text = try? c.decodeIfPresent(String.self, forKey: .key) {
                key = text
            } else if let number = try? c.decodeIfPresent(Double.self, forKey: .key) {
                key = number == number.rounded() ? String(Int(number)) : String(number)
            } else {
                key = ""
            }
            data = try c.decodeIfPresent(JSONValue.self, forKey: .data)
        }
    }

    let turn: Turn
    let conversation: ConversationRef
    let agent: AgentRef?
    let state: TimelineState
    let summary: String?
    /// How many messages the conversation has after this one.
    let laterTurns: Int
    let steps: [Step]
    let sources: [Source]
    let followups: [Suggestion]
    let followupDecisions: [FollowUpDecision]
    let memories: [Memory]
    let records: [Record]
    /// The workspace files that changed since the turn was sent.
    let files: [WorkspaceFile]
    /// Every call the run held, settled ones too.
    let held: [OutboxItem]

    /// The page's headline: why it failed, else the gist, else what was asked.
    var headline: String {
        if state == .failed {
            let error = (turn.error ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
            return error.isEmpty ? L10n.string("Cedar couldn't finish this.") : error
        }
        let summary = (self.summary ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !summary.isEmpty { return summary }
        let title = (conversation.title ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        return title.isEmpty ? turn.prompt : title
    }

    /// The answer as the page shows it under the headline: without its first paragraph when
    /// that is the gist the headline already says. `nil` when nothing is left to show.
    var answer: String? {
        guard let text = turn.finalText else { return nil }
        let rest = state == .failed ? text : Self.withoutHeadline(text, summary: summary)
        return rest.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : rest
    }

    /// `answer` without its first paragraph of prose when that reads the same as `summary`
    /// (the server takes the summary from it), and without a heading left with nothing under it.
    /// Anything else, a summary that was cut short included, leaves the answer as it is.
    static func withoutHeadline(_ answer: String, summary: String?) -> String {
        let gist = words(summary ?? "")
        guard !gist.isEmpty else { return answer }
        var blocks = Self.blocks(answer)
        guard let first = blocks.firstIndex(where: { isProse($0) }) else { return answer }
        // The summary is the first prose: a list, a table or code before it is not skipped over.
        guard blocks[..<first].allSatisfy({ isHeading($0) }) else { return answer }
        guard words(plain(blocks[first].joined(separator: " "))) == gist else { return answer }
        blocks.remove(at: first)
        let after = first < blocks.count ? blocks[first] : nil
        if first > 0, isHeading(blocks[first - 1]), after == nil || isHeading(after ?? []) {
            blocks.remove(at: first - 1)
        }
        return blocks.map { $0.joined(separator: "\n") }.joined(separator: "\n\n")
    }

    /// The answer's blocks, each its lines: split at blank lines, a heading on its own, and a
    /// fenced code block kept whole.
    private static func blocks(_ text: String) -> [[String]] {
        var blocks: [[String]] = []
        var current: [String] = []
        var inFence = false
        func flush() {
            if !current.isEmpty { blocks.append(current) }
            current = []
        }
        for line in text.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("```") {
                if !inFence { flush() }
                current.append(line)
                inFence.toggle()
                if !inFence { flush() }
            } else if inFence {
                current.append(line)
            } else if trimmed.isEmpty {
                flush()
            } else if trimmed.hasPrefix("#") {
                flush()
                blocks.append([line])
            } else {
                current.append(line)
            }
        }
        flush()
        return blocks
    }

    private static func isHeading(_ block: [String]) -> Bool {
        block.count == 1 && block[0].trimmingCharacters(in: .whitespaces).hasPrefix("#")
    }

    /// A paragraph: not a heading, a list, a quote, a table, a rule or code.
    private static func isProse(_ block: [String]) -> Bool {
        guard let first = block.first?.trimmingCharacters(in: .whitespaces), !first.isEmpty else { return false }
        if first.hasPrefix("#") || first.hasPrefix("```") || first.hasPrefix(">") || first.hasPrefix("|") || first.hasPrefix("---") {
            return false
        }
        if first.hasPrefix("- ") || first.hasPrefix("* ") || first.hasPrefix("+ ") { return false }
        // "1. " or "2) ": an ordered list.
        let digits = first.prefix { $0.isNumber }
        if !digits.isEmpty {
            let rest = first.dropFirst(digits.count)
            if rest.hasPrefix(". ") || rest.hasPrefix(") ") { return false }
        }
        return true
    }

    /// Markdown's inline marks taken out, as the server does for the summary: emphasis and code
    /// marks go, a link is its text, and a `[n]` citation goes with the space before it.
    private static func plain(_ text: String) -> String {
        let chars = Array(text)
        var out = ""
        var i = 0
        while i < chars.count {
            let c = chars[i]
            if c == "*" || c == "_" || c == "`" {
                i += 1
            } else if c == "[", let close = chars[i...].firstIndex(of: "]") {
                let inner = chars[(i + 1)..<close]
                if close + 1 < chars.count, chars[close + 1] == "(" {
                    out.append(contentsOf: inner)
                    if let end = chars[close...].firstIndex(of: ")") { i = end + 1 } else { i = close + 1 }
                } else if !inner.isEmpty, inner.allSatisfy({ $0.isASCII && $0.isNumber }) {
                    while out.hasSuffix(" ") { out.removeLast() }
                    i = close + 1
                } else {
                    out.append(c)
                    i += 1
                }
            } else {
                out.append(c)
                i += 1
            }
        }
        return out
    }

    /// Text with its runs of whitespace as single spaces, for comparing.
    private static func words(_ text: String) -> String {
        text.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }

    /// What was decided about a suggestion; `nil` while it is open.
    func decision(for suggestion: Suggestion) -> FollowUpDecision? {
        followupDecisions.first { $0.callId == suggestion.callId }
    }

    /// The held calls, those still waiting for an OK first.
    var heldOpenFirst: [OutboxItem] {
        held.filter(\.isOpen) + held.filter { !$0.isOpen }
    }

    enum CodingKeys: String, CodingKey {
        case turn, conversation, agent, state, summary, steps, sources, followups, memories, records, files, held
        case laterTurns = "later_turns"
        case followupDecisions = "followup_decisions"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        turn = try c.decode(Turn.self, forKey: .turn)
        conversation = try c.decode(ConversationRef.self, forKey: .conversation)
        agent = try c.decodeIfPresent(AgentRef.self, forKey: .agent)
        state = TimelineState(server: try c.decodeIfPresent(String.self, forKey: .state) ?? "")
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        laterTurns = try c.decodeIfPresent(Int.self, forKey: .laterTurns) ?? 0
        steps = try c.decodeIfPresent([Step].self, forKey: .steps) ?? []
        sources = try c.decodeIfPresent([Source].self, forKey: .sources) ?? []
        followups = try c.decodeIfPresent([Suggestion].self, forKey: .followups) ?? []
        followupDecisions = try c.decodeIfPresent([FollowUpDecision].self, forKey: .followupDecisions) ?? []
        memories = try c.decodeIfPresent([Memory].self, forKey: .memories) ?? []
        records = try c.decodeIfPresent([Record].self, forKey: .records) ?? []
        files = try c.decodeIfPresent([WorkspaceFile].self, forKey: .files) ?? []
        held = try c.decodeIfPresent([OutboxItem].self, forKey: .held) ?? []
    }
}

// MARK: Asking with a When

/// When an ask runs: now (a conversation), or as an agent later, on a clock, or when something
/// happens. The raw values are the server's.
enum AskWhen: String, CaseIterable, Identifiable, Hashable, Sendable {
    case now, later, every, watch

    var id: String { rawValue }

    var title: String {
        switch self {
        case .now: return L10n.string("Now")
        case .later: return L10n.string("Later")
        case .every: return L10n.string("Every…")
        case .watch: return L10n.string("When something changes")
        }
    }

    /// What the button that sends it says.
    var sendTitle: String {
        switch self {
        case .now: return L10n.string("Send")
        case .later: return L10n.string("Schedule")
        case .every: return L10n.string("Make It Standing")
        case .watch: return L10n.string("Start Watching")
        }
    }

    /// One line on what happens next.
    var hint: String {
        switch self {
        case .now: return L10n.string("Runs as soon as you send it.")
        case .later: return L10n.string("Runs once at the time you pick. It waits under Ahead until then.")
        case .every: return L10n.string("Becomes a standing ask. Each run shows up in Today when it is due.")
        case .watch: return L10n.string("Cedar keeps watch and runs it when that happens. Until then it waits under Ahead.")
        }
    }
}

/// How often an ask with "Every…" runs: the `time` trigger's frequencies.
enum AskFrequency: String, CaseIterable, Identifiable, Hashable, Sendable {
    case daily, weekdays, weekly

    var id: String { rawValue }

    var title: String {
        switch self {
        case .daily: return L10n.string("Every day")
        case .weekdays: return L10n.string("On weekdays")
        case .weekly: return L10n.string("Every week")
        }
    }
}

/// What an ask with "When something changes" waits for.
enum AskWatch: String, CaseIterable, Identifiable, Hashable, Sendable {
    case email, calendar, device

    var id: String { rawValue }

    var title: String {
        switch self {
        case .email: return L10n.string("When mail arrives that matches")
        case .calendar: return L10n.string("Before each calendar event")
        case .device: return L10n.string("When my phone or a shortcut asks")
        }
    }
}

/// An ask as its sheet holds it: the sentence, the When, and what each When needs.
struct AskDraft: Hashable, Sendable {
    var text = ""
    var when: AskWhen = .now
    /// Off the record: a private chat. Only with `now`.
    var offTheRecord = false
    /// Later: the instant it runs.
    var laterAt: Date
    var frequency: AskFrequency = .daily
    /// 0–6 for Monday–Sunday; weekly only.
    var weekday = 0
    /// Every…: the time of day, as an instant today.
    var time: Date
    var watch: AskWatch = .email
    /// Gmail's search syntax.
    var query = ""
    var minutesBefore = 30
    /// This device's IANA zone, which the times are in.
    var timezone = TimeZone.current.identifier
    /// A trigger Cedar read out of the sentence that the fields above can't show (monthly, every
    /// so many minutes): sent as it is while the When stays `keptWhen`.
    var kept: Trigger?
    var keptWhen: AskWhen?

    init(now: Date = Date(), timezone: String = TimeZone.current.identifier) {
        self.timezone = timezone
        // The hour is the zone's the times are in: this device's, unless a test says otherwise.
        var calendar = Calendar.current
        calendar.timeZone = TimeZone(identifier: timezone) ?? .current
        laterAt = Self.defaultLater(now: now, calendar: calendar)
        time = AgentClock.date(time: "07:00", timezone: timezone, on: now) ?? now
    }

    /// Two hours from `now`, on the hour as this device's clock shows it (which is not a round
    /// UTC hour in a zone half an hour off).
    static func defaultLater(now: Date, calendar: Calendar = .current) -> Date {
        let later = now.addingTimeInterval(2 * 3600)
        return calendar.dateInterval(of: .hour, for: later)?.start ?? later
    }

    /// The trigger kept from a reading, while the When is still the one it was read for.
    var keptTrigger: Trigger? {
        guard let kept, keptWhen == when else { return nil }
        return kept
    }

    /// Takes what Cedar read out of the sentence (`POST /api/asks/draft`): the When, the
    /// sentence without its timing, and the trigger. A trigger the fields can show fills them
    /// in; another is kept as it was read. `now` is for the day a time of day is put on.
    mutating func apply(_ reading: AskReading, now: Date = Date()) {
        let content = reading.content.trimmingCharacters(in: .whitespacesAndNewlines)
        if !content.isEmpty { text = content }
        kept = nil
        keptWhen = nil
        guard let read = AskWhen(rawValue: reading.when), read != .now, let trigger = reading.trigger else {
            when = .now
            return
        }
        when = read
        offTheRecord = false
        switch trigger {
        case .once(let at, let zone):
            if let date = AgentClock.date(at: at, timezone: zone) { laterAt = date } else { keep(trigger) }
        case .time(let frequency, let weekday, let time, let zone):
            if let known = AskFrequency(rawValue: frequency), let date = AgentClock.date(time: time, timezone: zone, on: now) {
                self.frequency = known
                self.weekday = min(max(weekday ?? 0, 0), 6)
                self.time = date
            } else {
                keep(trigger)
            }
        case .email(let query, _):
            watch = .email
            self.query = query
        case .calendar(let calendarId, let filter, let minutes?, let newEvents, _) where calendarId == nil && filter == nil && !newEvents:
            watch = .calendar
            minutesBefore = max(1, minutes)
        default:
            keep(trigger)
        }
    }

    private mutating func keep(_ trigger: Trigger) {
        kept = trigger
        keptWhen = when
    }

    var content: String { text.trimmingCharacters(in: .whitespacesAndNewlines) }

    /// What starts the agent's runs, for every When but `now`. `checkMinutes` is how often a
    /// watching trigger checks (the server's default).
    func trigger(checkMinutes: Int = 5) -> Trigger? {
        if let keptTrigger { return keptTrigger }
        switch when {
        case .now:
            return nil
        case .later:
            return .once(at: AgentClock.at(laterAt, timezone: timezone), timezone: timezone)
        case .every:
            return .time(frequency: frequency.rawValue, weekday: frequency == .weekly ? weekday : nil,
                         time: AgentClock.time(time, timezone: timezone), timezone: timezone)
        case .watch:
            switch watch {
            case .email:
                return .email(query: query.trimmingCharacters(in: .whitespacesAndNewlines), everyMinutes: checkMinutes)
            case .calendar:
                return .calendar(calendarId: nil, filter: nil, minutesBefore: max(1, minutesBefore), newEvents: false,
                                 everyMinutes: checkMinutes)
            case .device:
                return .device
            }
        }
    }

    /// Why it can't be sent yet, for the sheet to say; `nil` when it can.
    func problem(now: Date = Date()) -> String? {
        if content.isEmpty { return L10n.string("Say what Cedar should do.") }
        // A trigger kept as it was read was checked by the server.
        if keptTrigger != nil { return nil }
        switch when {
        case .later where laterAt <= now:
            return L10n.string("Pick a time that is still ahead.")
        case .watch where watch == .email && query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty:
            return L10n.string("Say which mail to watch for, as you would search for it in Gmail.")
        default:
            return nil
        }
    }

    /// The body `POST /api/asks` takes. `model`, `reasoning` and `plan` go with an ask that runs
    /// now, as with a message; an agent takes the server's defaults.
    func body(locale: String? = nil, model: String? = nil, reasoning: String? = nil, plan: String? = nil,
              checkMinutes: Int = 5) -> AskBody {
        if when == .now {
            return AskBody(content: content, isPrivate: offTheRecord ? true : nil, model: model, reasoning: reasoning,
                           plan: plan, locale: locale, timezone: timezone)
        }
        return AskBody(content: content, when: when.rawValue, trigger: trigger(checkMinutes: checkMinutes), timezone: timezone)
    }
}

/// The body of `POST /api/asks`. The server refuses fields it doesn't know, so only what is set
/// is written: `when` and `trigger` for an ask that becomes an agent, `private` for one off the
/// record.
struct AskBody: Encodable, Hashable, Sendable {
    var content: String
    var when: String?
    var trigger: Trigger?
    var isPrivate: Bool?
    var model: String?
    var reasoning: String?
    var plan: String?
    var locale: String?
    var timezone: String?

    enum CodingKeys: String, CodingKey {
        case content, when, trigger, model, reasoning, plan, locale, timezone
        case isPrivate = "private"
    }
}

/// The body of `POST /api/asks/draft`: the sentence, and the zone its times are in.
struct AskReadingBody: Encodable, Hashable, Sendable {
    var content: String
    var timezone: String
}

/// The answer to `POST /api/asks/draft`: the When Cedar read out of a sentence, the trigger
/// for it (`nil` for now) and the sentence without its timing. Nothing was made of it.
struct AskReading: Decodable, Hashable, Sendable {
    let when: String
    let trigger: Trigger?
    let content: String

    enum CodingKeys: String, CodingKey {
        case when, trigger, content
    }

    init(when: String, trigger: Trigger?, content: String) {
        self.when = when
        self.trigger = trigger
        self.content = content
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        when = try c.decodeIfPresent(String.self, forKey: .when) ?? "now"
        // A trigger this app can't read is no reading: the person picks the When.
        trigger = try? c.decodeIfPresent(Trigger.self, forKey: .trigger)
        content = try c.decodeIfPresent(String.self, forKey: .content) ?? ""
    }
}

/// `GET /api/search`: what else has the words, beside the conversations: agents, what Cedar
/// remembers, and record sets. A part the server can't give is empty.
struct SearchExtras: Decodable, Hashable, Sendable {
    struct Memory: Decodable, Identifiable, Hashable, Sendable {
        let id: String
        let text: String
    }

    struct RecordSet: Decodable, Identifiable, Hashable, Sendable {
        let name: String
        let description: String?
        var id: String { name }
    }

    let agents: [TimelineAgent]
    let memories: [Memory]
    let records: [RecordSet]

    var isEmpty: Bool { agents.isEmpty && memories.isEmpty && records.isEmpty }

    enum CodingKeys: String, CodingKey {
        case agents, memories, records
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agents = try c.decodeIfPresent([TimelineAgent].self, forKey: .agents) ?? []
        memories = try c.decodeIfPresent([Memory].self, forKey: .memories) ?? []
        records = try c.decodeIfPresent([RecordSet].self, forKey: .records) ?? []
    }
}

/// The answer to `POST /api/asks`: the new conversation and its first turn for an ask that runs
/// now, or the agent it became.
struct AskAnswer: Decodable, Sendable {
    let when: String
    let conversation: Conversation?
    let turn: Turn?
    let agent: Agent?

    enum CodingKeys: String, CodingKey {
        case when, conversation, turn, agent
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        when = try c.decodeIfPresent(String.self, forKey: .when) ?? "now"
        conversation = try c.decodeIfPresent(Conversation.self, forKey: .conversation)
        turn = try c.decodeIfPresent(Turn.self, forKey: .turn)
        agent = try c.decodeIfPresent(Agent.self, forKey: .agent)
    }
}
