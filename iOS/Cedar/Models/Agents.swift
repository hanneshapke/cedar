import Foundation

// Agents (`/api/agents`): prompts the server runs on its own, each with a trigger that says what
// starts a run: a clock, something it watches (a value, a record set, mail, a calendar, a Drive
// folder), a webhook, or the user's own device. Field names are the server's, in snake_case.
// What a client may leave out or get `null` for is optional here, so an agent from a newer
// server still lists.

/// What starts an agent's run, told apart by `kind`. A kind this version doesn't know (or one
/// whose fields it can't read) decodes as `.unknown` rather than failing, so the list still
/// loads; such an agent is shown read-only.
enum Trigger: Hashable, Sendable {
    /// `daily`, `weekdays` or `weekly` (on `weekday`, 0–6 for Monday–Sunday) at `time` (`HH:MM`).
    case time(frequency: String, weekday: Int?, time: String, timezone: String)
    /// One run at `at` (`YYYY-MM-DDTHH:MM`, local to `timezone`); the agent switches itself off after.
    case once(at: String, timezone: String)
    case interval(everyMinutes: Int)
    /// On `day` of every month (the last day in a shorter month) at `time`.
    case monthly(day: Int, time: String, timezone: String)
    /// A run when the value at `extract` in what `source` returns passes `predicate`.
    case condition(source: Source, extract: String?, predicate: Predicate, everyMinutes: Int)
    case webhook
    /// A run when mail matching `query` (Gmail's search syntax; empty for any mail) arrives.
    case email(query: String, everyMinutes: Int)
    /// A run `minutesBefore` an event starts, when a new event appears, or both.
    case calendar(calendarId: String?, filter: String?, minutesBefore: Int?, newEvents: Bool, everyMinutes: Int)
    /// A run when record set `set` gets new rows (`on` is `added`), or new or changed ones
    /// (`changed`). The run is handed those rows.
    case records(set: String, on: String, everyMinutes: Int)
    /// A run when files appear in a Google Drive folder: `folder` is its ID (the server also
    /// takes the folder's browser address and keeps the ID).
    case drive(folder: String, everyMinutes: Int)
    /// Waits for the user's own device to ask for a run (`POST /api/agents/{id}/trigger`).
    case device
    case unknown(kind: String)

    /// What a condition watches.
    enum Source: Hashable, Sendable {
        /// A web address, read as `json` or `text` (`format` is the JSON's `as`).
        case url(url: String, format: String?)
        /// One of cedar's tools that can be called outside a run, with this input.
        case tool(name: String, input: JSONValue)
        /// One of the record sets cedar keeps, observed as `{count, updated_at, records}`.
        case records(set: String)
    }

    /// The test a watched value has to pass: `changes` (no value), `gt` `lt` `gte` `lte` (a
    /// number), `equals` or `contains` (text, a number or a boolean).
    struct Predicate: Hashable, Sendable {
        var op: String
        var value: JSONValue?

        static let ops = ["changes", "gt", "gte", "lt", "lte", "equals", "contains"]
        static let numberOps: Set<String> = ["gt", "gte", "lt", "lte"]
    }

    /// How the Agents list groups triggers.
    enum Group: String, CaseIterable, Identifiable, Sendable {
        /// `outside` is what is started from outside cedar: a webhook's call, or the user's device.
        case schedule, change, outside, mailCalendarFiles, other

        var id: String { rawValue }

        var title: String {
            switch self {
            case .schedule: return L10n.string("On a schedule")
            case .change: return L10n.string("When something changes")
            case .outside: return L10n.string("Webhooks and devices")
            case .mailCalendarFiles: return L10n.string("Mail, calendar and files")
            case .other: return L10n.string("Other")
            }
        }
    }

    /// The longest interval, and the longest time between two checks: a week.
    static let maxEveryMinutes = 7 * 24 * 60

    var kind: String {
        switch self {
        case .time: return "time"
        case .once: return "once"
        case .interval: return "interval"
        case .monthly: return "monthly"
        case .condition: return "condition"
        case .webhook: return "webhook"
        case .email: return "email"
        case .calendar: return "calendar"
        case .records: return "records"
        case .drive: return "drive"
        case .device: return "device"
        case .unknown(let kind): return kind
        }
    }

    var isKnown: Bool {
        if case .unknown = self { return false }
        return true
    }

    var group: Group { Self.group(ofKind: kind) }

    static func group(ofKind kind: String) -> Group {
        switch kind {
        case "time", "once", "interval", "monthly": return .schedule
        case "condition", "records": return .change
        case "webhook", "device": return .outside
        case "email", "calendar", "drive": return .mailCalendarFiles
        default: return .other
        }
    }

    /// The clock starts its runs.
    var isClock: Bool { group == .schedule }

    /// The server checks something for it between runs.
    var watches: Bool { Self.watches(kind: kind) }

    static func watches(kind: String) -> Bool {
        kind == "condition" || kind == "email" || kind == "calendar" || kind == "records" || kind == "drive"
    }

    /// Something outside starts its runs (a value, mail, an event, a call, the user's device)
    /// rather than the clock: these get the limits on how often, and the careful choice of tools.
    var byEvent: Bool { Self.byEvent(kind: kind) }

    static func byEvent(kind: String) -> Bool {
        watches(kind: kind) || kind == "webhook" || kind == "device"
    }

    /// The time zone its times are in, for the triggers that have one.
    var timezone: String? {
        switch self {
        case .time(_, _, _, let timezone), .once(_, let timezone), .monthly(_, _, let timezone): return timezone
        default: return nil
        }
    }

    /// The SF Symbol a row shows for it.
    var systemImage: String { Self.systemImage(forKind: kind) }

    static func systemImage(forKind kind: String) -> String {
        switch kind {
        case "time", "monthly": return "clock"
        case "once": return "calendar"
        case "interval": return "repeat"
        case "condition": return "eye"
        case "webhook": return "link"
        case "email": return "envelope"
        case "calendar": return "calendar.badge.clock"
        case "records": return "tablecells"
        case "drive": return "folder"
        case "device": return "iphone"
        default: return "questionmark.circle"
        }
    }

    /// One line saying what starts the agent: "Weekdays at 07:30", "Every 2 hours",
    /// "api.example.com: price > 100", with a time zone when it isn't this device's.
    var summary: String {
        switch self {
        case .time(let frequency, let weekday, let time, let timezone):
            let when: String
            switch frequency {
            case "daily": when = L10n.string("Daily")
            case "weekdays": when = L10n.string("Weekdays")
            case "weekly":
                let names = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays"]
                when = L10n.string(weekday.flatMap { names.indices.contains($0) ? names[$0] : nil } ?? "Weekly")
            default: when = frequency.capitalized
            }
            return L10n.format("%@ at %@", when, time) + Self.zoneSuffix(timezone)
        case .once(let at, let timezone):
            guard let date = AgentClock.date(at: at, timezone: timezone) else {
                return L10n.format("Once, %@", at) + Self.zoneSuffix(timezone)
            }
            let zone = TimeZone(identifier: timezone) ?? .current
            let style = Date.FormatStyle(date: .abbreviated, time: .shortened, locale: L10n.locale, timeZone: zone)
            return L10n.format("Once, %@", date.formatted(style)) + Self.zoneSuffix(timezone)
        case .interval(let everyMinutes):
            return Self.every(everyMinutes)
        case .monthly(let day, let time, let timezone):
            return L10n.format("Day %lld of every month at %@", day, time) + Self.zoneSuffix(timezone)
        case .condition(let source, let extract, let predicate, _):
            let path = (extract ?? "").trimmingCharacters(in: .whitespaces)
            let subject = path.isEmpty ? L10n.string("the value") : path
            return L10n.labeled(source.label, predicate.text(subject: subject))
        case .webhook:
            return L10n.string("When its webhook is called")
        case .email(let query, _):
            let query = query.trimmingCharacters(in: .whitespaces)
            return query.isEmpty ? L10n.string("Any new mail") : L10n.format("New mail matching %@", query)
        case .calendar(_, let filter, let minutesBefore, let newEvents, _):
            var parts: [String] = []
            if let minutesBefore {
                parts.append(minutesBefore == 0
                             ? L10n.string("When an event starts")
                             : L10n.format("%@ before an event", Self.duration(minutesBefore)))
            }
            if newEvents { parts.append(L10n.string("When an event is added")) }
            if parts.isEmpty { parts.append(L10n.string("Calendar events")) }
            if let filter, !filter.trimmingCharacters(in: .whitespaces).isEmpty {
                parts.append(L10n.format("matching %@", filter))
            }
            return parts.joined(separator: " · ")
        case .records(let set, let on, _):
            return L10n.format(on == "changed" ? "New or changed rows in %@" : "New rows in %@", set)
        case .drive:
            return L10n.string("New files in a Google Drive folder")
        case .device:
            return L10n.string("When your device asks")
        case .unknown(let kind):
            return kind.isEmpty
                ? L10n.string("A trigger this version of the app doesn't know")
                : L10n.format("A trigger this version of the app doesn't know (%@)", kind)
        }
    }

    /// "Every 5 minutes", "Every hour", "Every 2 days": the largest unit that divides it.
    static func every(_ minutes: Int) -> String {
        if minutes > 0, minutes % 1440 == 0 {
            let days = minutes / 1440
            return days == 1 ? L10n.string("Every day") : L10n.format("Every %lld days", days)
        }
        if minutes > 0, minutes % 60 == 0 {
            let hours = minutes / 60
            return hours == 1 ? L10n.string("Every hour") : L10n.format("Every %lld hours", hours)
        }
        return minutes == 1 ? L10n.string("Every minute") : L10n.format("Every %lld minutes", minutes)
    }

    /// "15 minutes", "1 hour", "2 days": the largest unit that divides it.
    static func duration(_ minutes: Int) -> String {
        if minutes > 0, minutes % 1440 == 0 {
            let days = minutes / 1440
            return L10n.format(days == 1 ? "%lld day" : "%lld days", days)
        }
        if minutes > 0, minutes % 60 == 0 {
            let hours = minutes / 60
            return L10n.format(hours == 1 ? "%lld hour" : "%lld hours", hours)
        }
        return L10n.format(minutes == 1 ? "%lld minute" : "%lld minutes", minutes)
    }

    /// " (Europe/Berlin)" when the zone isn't this device's.
    private static func zoneSuffix(_ timezone: String) -> String {
        timezone == TimeZone.current.identifier ? "" : " (\(timezone))"
    }
}

extension Trigger.Source {
    /// `url`, `tool` or `records`, as the JSON's `type`.
    var type: String {
        switch self {
        case .url: return "url"
        case .tool: return "tool"
        case .records: return "records"
        }
    }

    /// What a row calls it: a web address's host, a tool's name, a record set's name.
    var label: String {
        switch self {
        case .url(let url, _):
            return URL(string: url)?.host ?? url
        case .tool(let name, _):
            return name
        case .records(let set):
            return L10n.format("Records %@", set)
        }
    }
}

extension Trigger.Predicate {
    /// The value as a person would write it: text without quotes, 40 rather than 40.0.
    var valueText: String {
        guard let value else { return "" }
        switch value {
        case .string(let text): return text
        case .number(let number):
            if number.isFinite, number == number.rounded(), abs(number) < 1e15 { return String(Int64(number)) }
            return String(number)
        case .bool(let flag): return flag ? "true" : "false"
        case .null: return ""
        case .array, .object: return value.prettyPrinted
        }
    }

    /// "price changes", "price > 100", "status contains shipped".
    func text(subject: String) -> String {
        switch op {
        case "changes": return L10n.format("%@ changes", subject)
        case "gt": return "\(subject) > \(valueText)"
        case "gte": return "\(subject) ≥ \(valueText)"
        case "lt": return "\(subject) < \(valueText)"
        case "lte": return "\(subject) ≤ \(valueText)"
        case "equals": return "\(subject) = \(valueText)"
        case "contains": return L10n.format("%@ contains %@", subject, valueText)
        default: return "\(subject) \(op) \(valueText)"
        }
    }

    /// How the form's picker names an operator.
    static func label(op: String) -> String {
        switch op {
        case "changes": return L10n.string("changes")
        case "gt": return L10n.string("is greater than")
        case "gte": return L10n.string("is at least")
        case "lt": return L10n.string("is less than")
        case "lte": return L10n.string("is at most")
        case "equals": return L10n.string("equals")
        case "contains": return L10n.string("contains")
        default: return op
        }
    }
}

// MARK: Trigger JSON

extension Trigger: Codable {
    private enum CodingKeys: String, CodingKey {
        case kind, frequency, weekday, time, timezone, at, day, source, extract, predicate, query, filter, set, on, folder
        case everyMinutes = "every_minutes"
        case calendarId = "calendar_id"
        case minutesBefore = "minutes_before"
        case newEvents = "new_events"
    }

    /// How often a watching trigger checks when it doesn't say.
    private static let defaultCheckMinutes = 5

    /// Never throws: whatever can't be read is `.unknown`, with the kind the server named.
    init(from decoder: Decoder) throws {
        guard let c = try? decoder.container(keyedBy: CodingKeys.self) else {
            self = .unknown(kind: "")
            return
        }
        let kind = (try? c.decode(String.self, forKey: .kind)) ?? ""
        do {
            self = try Trigger.read(kind: kind, from: c)
        } catch {
            self = .unknown(kind: kind)
        }
    }

    private static func read(kind: String, from c: KeyedDecodingContainer<CodingKeys>) throws -> Trigger {
        switch kind {
        case "time":
            return .time(frequency: try c.decode(String.self, forKey: .frequency),
                         weekday: try c.decodeIfPresent(Int.self, forKey: .weekday),
                         time: try c.decode(String.self, forKey: .time),
                         timezone: try c.decode(String.self, forKey: .timezone))
        case "once":
            return .once(at: try c.decode(String.self, forKey: .at),
                         timezone: try c.decode(String.self, forKey: .timezone))
        case "interval":
            return .interval(everyMinutes: try c.decode(Int.self, forKey: .everyMinutes))
        case "monthly":
            return .monthly(day: try c.decode(Int.self, forKey: .day),
                            time: try c.decode(String.self, forKey: .time),
                            timezone: try c.decode(String.self, forKey: .timezone))
        case "condition":
            return .condition(source: try c.decode(Source.self, forKey: .source),
                              extract: try c.decodeIfPresent(String.self, forKey: .extract),
                              predicate: try c.decode(Predicate.self, forKey: .predicate),
                              everyMinutes: try c.decodeIfPresent(Int.self, forKey: .everyMinutes) ?? defaultCheckMinutes)
        case "webhook":
            return .webhook
        case "email":
            return .email(query: try c.decodeIfPresent(String.self, forKey: .query) ?? "",
                          everyMinutes: try c.decodeIfPresent(Int.self, forKey: .everyMinutes) ?? defaultCheckMinutes)
        case "calendar":
            return .calendar(calendarId: try c.decodeIfPresent(String.self, forKey: .calendarId),
                             filter: try c.decodeIfPresent(String.self, forKey: .filter),
                             minutesBefore: try c.decodeIfPresent(Int.self, forKey: .minutesBefore),
                             newEvents: try c.decodeIfPresent(Bool.self, forKey: .newEvents) ?? false,
                             everyMinutes: try c.decodeIfPresent(Int.self, forKey: .everyMinutes) ?? defaultCheckMinutes)
        case "records":
            return .records(set: try c.decode(String.self, forKey: .set),
                            on: try c.decodeIfPresent(String.self, forKey: .on) ?? "added",
                            everyMinutes: try c.decodeIfPresent(Int.self, forKey: .everyMinutes) ?? defaultCheckMinutes)
        case "drive":
            return .drive(folder: try c.decode(String.self, forKey: .folder),
                          everyMinutes: try c.decodeIfPresent(Int.self, forKey: .everyMinutes) ?? defaultCheckMinutes)
        case "device":
            return .device
        default:
            return .unknown(kind: kind)
        }
    }

    /// The JSON the server takes: it refuses fields it doesn't know, so only the kind's own are
    /// written, and an optional one that isn't set is left out.
    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(kind, forKey: .kind)
        switch self {
        case .time(let frequency, let weekday, let time, let timezone):
            try c.encode(frequency, forKey: .frequency)
            try c.encodeIfPresent(weekday, forKey: .weekday)
            try c.encode(time, forKey: .time)
            try c.encode(timezone, forKey: .timezone)
        case .once(let at, let timezone):
            try c.encode(at, forKey: .at)
            try c.encode(timezone, forKey: .timezone)
        case .interval(let everyMinutes):
            try c.encode(everyMinutes, forKey: .everyMinutes)
        case .monthly(let day, let time, let timezone):
            try c.encode(day, forKey: .day)
            try c.encode(time, forKey: .time)
            try c.encode(timezone, forKey: .timezone)
        case .condition(let source, let extract, let predicate, let everyMinutes):
            try c.encode(source, forKey: .source)
            try c.encodeIfPresent(extract, forKey: .extract)
            try c.encode(predicate, forKey: .predicate)
            try c.encode(everyMinutes, forKey: .everyMinutes)
        case .webhook, .device, .unknown:
            break
        case .email(let query, let everyMinutes):
            try c.encode(query, forKey: .query)
            try c.encode(everyMinutes, forKey: .everyMinutes)
        case .calendar(let calendarId, let filter, let minutesBefore, let newEvents, let everyMinutes):
            try c.encodeIfPresent(calendarId, forKey: .calendarId)
            try c.encodeIfPresent(filter, forKey: .filter)
            try c.encodeIfPresent(minutesBefore, forKey: .minutesBefore)
            try c.encode(newEvents, forKey: .newEvents)
            try c.encode(everyMinutes, forKey: .everyMinutes)
        case .records(let set, let on, let everyMinutes):
            try c.encode(set, forKey: .set)
            try c.encode(on, forKey: .on)
            try c.encode(everyMinutes, forKey: .everyMinutes)
        case .drive(let folder, let everyMinutes):
            try c.encode(folder, forKey: .folder)
            try c.encode(everyMinutes, forKey: .everyMinutes)
        }
    }
}

extension Trigger.Source: Codable {
    private enum CodingKeys: String, CodingKey {
        case type, url, name, input, set
        /// `json` or `text`.
        case format = "as"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        let type = try c.decode(String.self, forKey: .type)
        switch type {
        case "url":
            self = .url(url: try c.decode(String.self, forKey: .url),
                        format: try c.decodeIfPresent(String.self, forKey: .format))
        case "tool":
            let input = try c.decodeIfPresent(JSONValue.self, forKey: .input) ?? .null
            self = .tool(name: try c.decode(String.self, forKey: .name),
                         input: input == .null ? .object([:]) : input)
        case "records":
            self = .records(set: try c.decode(String.self, forKey: .set))
        default:
            throw DecodingError.dataCorruptedError(forKey: .type, in: c, debugDescription: "unknown source \(type)")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(type, forKey: .type)
        switch self {
        case .url(let url, let format):
            try c.encode(url, forKey: .url)
            try c.encodeIfPresent(format, forKey: .format)
        case .tool(let name, let input):
            try c.encode(name, forKey: .name)
            try c.encode(input, forKey: .input)
        case .records(let set):
            try c.encode(set, forKey: .set)
        }
    }
}

extension Trigger.Predicate: Codable {
    private enum CodingKeys: String, CodingKey {
        case op, value
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        op = try c.decode(String.self, forKey: .op)
        let value = try c.decodeIfPresent(JSONValue.self, forKey: .value)
        self.value = value == .null ? nil : value
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(op, forKey: .op)
        try c.encodeIfPresent(value, forKey: .value)
    }
}

/// Times as triggers write them: `HH:MM` and `YYYY-MM-DDTHH:MM`, local to the trigger's zone.
enum AgentClock {
    private static func formatter(_ format: String, timezone: String) -> DateFormatter {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.calendar = Calendar(identifier: .gregorian)
        f.timeZone = TimeZone(identifier: timezone) ?? .current
        f.dateFormat = format
        return f
    }

    /// The instant a `once` trigger's `at` means.
    static func date(at: String, timezone: String) -> Date? {
        let text = at.trimmingCharacters(in: .whitespaces)
        return formatter("yyyy-MM-dd'T'HH:mm", timezone: timezone).date(from: text)
            ?? formatter("yyyy-MM-dd'T'HH:mm:ss", timezone: timezone).date(from: text)
    }

    /// `2026-12-03T09:00` for an instant, as the zone's clock shows it.
    static func at(_ date: Date, timezone: String) -> String {
        formatter("yyyy-MM-dd'T'HH:mm", timezone: timezone).string(from: date)
    }

    /// `09:30` for an instant, as the zone's clock shows it.
    static func time(_ date: Date, timezone: String) -> String {
        formatter("HH:mm", timezone: timezone).string(from: date)
    }

    /// Today at `time` (`HH:MM`) in the zone, for a time picker; `nil` when it isn't a time.
    static func date(time: String, timezone: String, on day: Date = Date()) -> Date? {
        let parts = time.trimmingCharacters(in: .whitespaces).split(separator: ":")
        guard parts.count >= 2, let hour = Int(parts[0]), let minute = Int(parts[1]),
              (0..<24).contains(hour), (0..<60).contains(minute) else { return nil }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: timezone) ?? .current
        return calendar.date(bySettingHour: hour, minute: minute, second: 0, of: day)
    }
}

// MARK: Agents and runs

/// An agent (`GET /api/agents`): a prompt the server starts as a fresh run whenever its trigger
/// says so, each run in a conversation of its own that the conversation list leaves out.
struct Agent: Decodable, Identifiable, Hashable, Sendable {
    /// A webhook agent's address. `secret` is there only in the answer to creating the agent or
    /// asking for a new secret; afterwards the server only tells its last four characters.
    struct Hook: Decodable, Hashable, Sendable {
        var url: String?
        var path: String?
        var secretLast4: String?
        var secret: String?

        enum CodingKeys: String, CodingKey {
            case url, path, secret
            case secretLast4 = "secret_last4"
        }
    }

    let id: String
    var title: String
    var prompt: String
    var model: String?
    var reasoning: String?
    /// The tools its runs use; `nil` for the server's defaults.
    var tools: [String]?
    /// `none` or a notification method; `nil` for the default one.
    var notify: String?
    var enabled: Bool
    var sourceConversationId: String?
    var timezone: String?
    var trigger: Trigger
    /// Clock triggers only; `nil` for a one-off that has run.
    var nextRunAt: Date?
    /// Watching triggers only.
    var nextCheckAt: Date?
    var lastRunAt: Date?
    var lastCheckedAt: Date?
    /// Why the last check failed.
    var lastError: String?
    /// A webhook delivery is held until the agent may run.
    var waiting: Bool
    /// How far apart events may start runs, and how many in 24 hours; `nil` for the defaults.
    var minIntervalSecs: Int?
    var maxRunsPerDay: Int?
    var hook: Hook?
    var lastRun: AgentRun?

    enum CodingKeys: String, CodingKey {
        case id, title, prompt, model, reasoning, tools, notify, enabled, timezone, trigger, waiting, hook
        case sourceConversationId = "source_conversation_id"
        case nextRunAt = "next_run_at"
        case nextCheckAt = "next_check_at"
        case lastRunAt = "last_run_at"
        case lastCheckedAt = "last_checked_at"
        case lastError = "last_error"
        case minIntervalSecs = "min_interval_secs"
        case maxRunsPerDay = "max_runs_per_day"
        case lastRun = "last_run"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decode(String.self, forKey: .title)
        prompt = try c.decodeIfPresent(String.self, forKey: .prompt) ?? ""
        model = try c.decodeIfPresent(String.self, forKey: .model)
        reasoning = try c.decodeIfPresent(String.self, forKey: .reasoning)
        tools = try c.decodeIfPresent([String].self, forKey: .tools)
        notify = try c.decodeIfPresent(String.self, forKey: .notify)
        enabled = try c.decodeIfPresent(Bool.self, forKey: .enabled) ?? true
        sourceConversationId = try c.decodeIfPresent(String.self, forKey: .sourceConversationId)
        timezone = try c.decodeIfPresent(String.self, forKey: .timezone)
        trigger = try c.decodeIfPresent(Trigger.self, forKey: .trigger) ?? .unknown(kind: "")
        nextRunAt = try c.decodeIfPresent(Date.self, forKey: .nextRunAt)
        nextCheckAt = try c.decodeIfPresent(Date.self, forKey: .nextCheckAt)
        lastRunAt = try c.decodeIfPresent(Date.self, forKey: .lastRunAt)
        lastCheckedAt = try c.decodeIfPresent(Date.self, forKey: .lastCheckedAt)
        lastError = try c.decodeIfPresent(String.self, forKey: .lastError)
        waiting = try c.decodeIfPresent(Bool.self, forKey: .waiting) ?? false
        minIntervalSecs = try c.decodeIfPresent(Int.self, forKey: .minIntervalSecs)
        maxRunsPerDay = try c.decodeIfPresent(Int.self, forKey: .maxRunsPerDay)
        hook = try c.decodeIfPresent(Hook.self, forKey: .hook)
        lastRun = try c.decodeIfPresent(AgentRun.self, forKey: .lastRun)
    }

    /// Where an agent stands, for its row. Derived as the server's contract says: paused (or
    /// done, for a one-off that has run) comes first, then a failing check, then what it is
    /// waiting for.
    enum Status: Hashable, Sendable {
        case paused
        /// A one-off that has run.
        case done
        case checkFailing(String)
        case nextRun(Date)
        case watching(lastChecked: Date?)
        case waitingForCalls
        /// A webhook delivery is held until the agent may run again.
        case callWaiting
        /// It runs when the user's own device asks.
        case waitingForDevice
        /// Nothing to say: a clock without a next run, or a trigger this version doesn't know.
        case idle
    }

    var status: Status {
        if !enabled {
            if case .once = trigger, nextRunAt == nil { return .done }
            return .paused
        }
        if let lastError, !lastError.isEmpty { return .checkFailing(lastError) }
        if trigger.watches { return .watching(lastChecked: lastCheckedAt) }
        if case .webhook = trigger { return waiting ? .callWaiting : .waitingForCalls }
        if case .device = trigger { return .waitingForDevice }
        if let nextRunAt { return .nextRun(nextRunAt) }
        return .idle
    }
}

/// One time an agent's run was started, with the turn that carries it, or the `error` that kept
/// it from starting.
struct AgentRun: Decodable, Identifiable, Hashable, Sendable {
    /// What started a run. `summary` is one line, already in the user's language.
    struct Cause: Decodable, Hashable, Sendable {
        /// `clock`, `changed`, `holds`, `webhook`, `mail`, `soon`, `new_event`, `rows`, `files` or
        /// `device`. Only `summary` is shown, so a kind from a newer server needs nothing here.
        let kind: String
        let summary: String
        /// The start of the data that came with it.
        let excerpt: String?

        enum CodingKeys: String, CodingKey {
            case kind, summary, excerpt
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            kind = try c.decodeIfPresent(String.self, forKey: .kind) ?? ""
            summary = try c.decodeIfPresent(String.self, forKey: .summary) ?? ""
            excerpt = try c.decodeIfPresent(String.self, forKey: .excerpt)
        }
    }

    let id: String
    /// The agent's id (the server's name for it is from before agents had other triggers).
    let scheduleId: String
    let conversationId: String?
    let turnId: String?
    let dueAt: Date
    let startedAt: Date
    /// Started with Run Now rather than by its trigger.
    let manual: Bool
    let error: String?
    /// `nil` for Run Now and for runs from before causes were kept.
    let cause: Cause?
    let turn: Turn?

    enum CodingKeys: String, CodingKey {
        case id, manual, error, cause, turn
        case scheduleId = "schedule_id"
        case conversationId = "conversation_id"
        case turnId = "turn_id"
        case dueAt = "due_at"
        case startedAt = "started_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        scheduleId = try c.decodeIfPresent(String.self, forKey: .scheduleId) ?? ""
        conversationId = try c.decodeIfPresent(String.self, forKey: .conversationId)
        turnId = try c.decodeIfPresent(String.self, forKey: .turnId)
        let started = try c.decode(Date.self, forKey: .startedAt)
        startedAt = started
        dueAt = try c.decodeIfPresent(Date.self, forKey: .dueAt) ?? started
        manual = try c.decodeIfPresent(Bool.self, forKey: .manual) ?? false
        error = try c.decodeIfPresent(String.self, forKey: .error)
        cause = try c.decodeIfPresent(Cause.self, forKey: .cause)
        turn = try c.decodeIfPresent(Turn.self, forKey: .turn)
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
    /// What started it, in the server's words; `nil` for Run Now and older runs.
    var startedBy: String? {
        guard let summary = cause?.summary.trimmingCharacters(in: .whitespacesAndNewlines), !summary.isEmpty else { return nil }
        return summary
    }
}

/// `GET /api/agents`: the agents, enabled first, and the server's limits and defaults.
struct AgentList: Decodable, Sendable {
    struct Limits: Decodable, Hashable, Sendable {
        var maxAgents = 100
        /// The defaults for an agent that sets none: how far apart events may start runs ...
        var minIntervalSecs = 300
        /// ... and how many in 24 hours.
        var maxRunsPerDay = 24
        /// The shortest interval trigger.
        var minIntervalMinutes = 15
        /// How often a watching trigger checks unless it says.
        var checkMinutes = 5
        /// A personal tool is checked no more often than this.
        var personalCheckMinutes = 5

        enum CodingKeys: String, CodingKey {
            case maxAgents = "max_agents"
            case minIntervalSecs = "min_interval_secs"
            case maxRunsPerDay = "max_runs_per_day"
            case minIntervalMinutes = "min_interval_minutes"
            case checkMinutes = "check_minutes"
            case personalCheckMinutes = "personal_check_minutes"
        }

        init() {}

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            maxAgents = try c.decodeIfPresent(Int.self, forKey: .maxAgents) ?? 100
            minIntervalSecs = try c.decodeIfPresent(Int.self, forKey: .minIntervalSecs) ?? 300
            maxRunsPerDay = try c.decodeIfPresent(Int.self, forKey: .maxRunsPerDay) ?? 24
            minIntervalMinutes = try c.decodeIfPresent(Int.self, forKey: .minIntervalMinutes) ?? 15
            checkMinutes = try c.decodeIfPresent(Int.self, forKey: .checkMinutes) ?? 5
            personalCheckMinutes = try c.decodeIfPresent(Int.self, forKey: .personalCheckMinutes) ?? 5
        }
    }

    let agents: [Agent]
    let limits: Limits

    enum CodingKeys: String, CodingKey {
        case agents, limits
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agents = try c.decode([Agent].self, forKey: .agents)
        limits = try c.decodeIfPresent(Limits.self, forKey: .limits) ?? Limits()
    }
}

/// `GET /api/agents/{id}`: the agent and its runs, newest first.
struct AgentDetail: Decodable, Sendable {
    let agent: Agent
    let runs: [AgentRun]

    enum CodingKeys: String, CodingKey {
        case agent, runs
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agent = try c.decode(Agent.self, forKey: .agent)
        runs = try c.decodeIfPresent([AgentRun].self, forKey: .runs) ?? []
    }
}

/// The agent a run or a conversation belongs to, as the activity feed and
/// `GET /api/conversations/{id}` name it.
struct AgentRef: Decodable, Hashable, Sendable {
    let id: String
    let title: String
    let triggerKind: String?

    enum CodingKeys: String, CodingKey {
        case id, title
        case triggerKind = "trigger_kind"
    }

    init(id: String, title: String, triggerKind: String? = nil) {
        self.id = id
        self.title = title
        self.triggerKind = triggerKind
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        title = try c.decodeIfPresent(String.self, forKey: .title) ?? ""
        triggerKind = try c.decodeIfPresent(String.self, forKey: .triggerKind)
    }
}

/// One run in the activity feed: a run's own fields, and `agent` beside them.
struct ActivityItem: Decodable, Identifiable, Hashable, Sendable {
    let run: AgentRun
    let agent: AgentRef?

    var id: String { run.id }

    enum CodingKeys: String, CodingKey {
        case agent
    }

    init(from decoder: Decoder) throws {
        run = try AgentRun(from: decoder)
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agent = try c.decodeIfPresent(AgentRef.self, forKey: .agent)
    }
}

/// `GET /api/agents/activity`: the latest runs of every agent, newest first. `nextBefore` goes
/// back as `before` for the next page, as the server wrote it; `nil` on the last page.
struct ActivityPage: Decodable, Sendable {
    let items: [ActivityItem]
    let nextBefore: String?

    enum CodingKeys: String, CodingKey {
        case items
        case nextBefore = "next_before"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        items = try c.decodeIfPresent([ActivityItem].self, forKey: .items) ?? []
        nextBefore = try c.decodeIfPresent(String.self, forKey: .nextBefore)
    }
}

/// `POST /api/agents/{id}/trigger`: what came of this device asking an agent for a run. It either
/// started, or is held back (`held` says why) and then isn't queued: nothing runs later.
struct AgentTriggerResult: Decodable, Hashable, Sendable {
    let started: Bool
    /// The run's conversation, when it started.
    let conversationId: String?
    /// `busy`, `too_soon`, `daily_limit` or `cedar_busy`, when it didn't.
    let held: String?

    enum CodingKeys: String, CodingKey {
        case started, held
        case conversationId = "conversation_id"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        started = try c.decodeIfPresent(Bool.self, forKey: .started) ?? false
        conversationId = try c.decodeIfPresent(String.self, forKey: .conversationId)
        held = try c.decodeIfPresent(String.self, forKey: .held)
    }

    /// One sentence for the person who asked: it started, or why it didn't.
    func message(agent title: String) -> String {
        if started { return L10n.format("%@ started.", title) }
        switch held {
        case "busy": return L10n.format("%@ is still busy with its last run, so no new run was started.", title)
        case "too_soon": return L10n.format("%@ ran a moment ago, and its limits don't allow another run yet.", title)
        case "daily_limit": return L10n.format("%@ has used up its runs for today.", title)
        case "cedar_busy": return L10n.format("Cedar is busy right now, so %@ was not started. Try again in a moment.", title)
        default: return L10n.format("%@ was not started.", title)
        }
    }
}

/// The body of `POST /api/agents/{id}/trigger`: where the request comes from (`shortcut`,
/// `share`, `automation`) and what is handed to the run as data. All of it is optional, and the
/// server refuses fields it doesn't know, so only what is set is written.
struct AgentTriggerBody: Encodable, Sendable {
    var source: String?
    var text: String?
    var title: String?
    var url: String?
}

/// What was decided about a follow-up the model suggested (`suggest_follow_up`), as
/// `GET /api/conversations/{id}` lists it and `PUT /api/turns/{turn_id}/followups/{call_id}`
/// answers. A suggestion without one is still open; the first decision stands.
struct FollowUpDecision: Decodable, Hashable, Sendable {
    let turnId: String
    /// The id of the tool call that made the suggestion.
    let callId: String
    /// `accepted` or `dismissed`.
    let status: String
    /// The one-off agent that was made, when it was accepted and the agent still exists.
    let agentId: String?
    let decidedAt: Date?

    var isAccepted: Bool { status == "accepted" }
    var isDismissed: Bool { status == "dismissed" }
    /// What tells one suggestion from another in a conversation: its turn and its tool call.
    var key: String { Self.key(turnId: turnId, callId: callId) }

    static func key(turnId: String, callId: String) -> String { "\(turnId)/\(callId)" }

    enum CodingKeys: String, CodingKey {
        case status
        case turnId = "turn_id"
        case callId = "call_id"
        case agentId = "agent_id"
        case decidedAt = "decided_at"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        turnId = try c.decode(String.self, forKey: .turnId)
        callId = try c.decode(String.self, forKey: .callId)
        status = try c.decodeIfPresent(String.self, forKey: .status) ?? ""
        agentId = try c.decodeIfPresent(String.self, forKey: .agentId)
        decidedAt = try c.decodeIfPresent(Date.self, forKey: .decidedAt)
    }
}

/// The answer to deciding a follow-up: the decision that stands (an earlier one, if there was
/// one), and the agent accepting made.
struct FollowUpAnswer: Decodable, Sendable {
    let followup: FollowUpDecision
    let agent: Agent?
}

/// The body of `PUT /api/turns/{turn_id}/followups/{call_id}`: `accept` with the suggestion as
/// the tool gave it and this device's time zone, or `dismiss` alone.
struct FollowUpBody: Encodable, Hashable, Sendable {
    let decision: String
    var title: String?
    var prompt: String?
    /// RFC 3339, as the tool wrote it.
    var at: String?
    var timezone: String?

    static let dismiss = FollowUpBody(decision: "dismiss")

    static func accept(_ followUp: FollowUp, timezone: String = TimeZone.current.identifier) -> FollowUpBody {
        FollowUpBody(decision: "accept", title: followUp.title, prompt: followUp.prompt, at: followUp.at, timezone: timezone)
    }
}

/// `POST /api/agents/test`: one check of a watching trigger, with nothing kept. Says what was
/// seen, so a path can be found in it, and whether a run would start.
struct TriggerTest: Decodable, Hashable, Sendable {
    /// What the check returned, pretty-printed when it is JSON, cut when long.
    struct Sample: Decodable, Hashable, Sendable {
        let text: String
        let cut: Bool
        let json: Bool

        enum CodingKeys: String, CodingKey {
            case text, cut, json
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            text = try c.decodeIfPresent(String.self, forKey: .text) ?? ""
            cut = try c.decodeIfPresent(Bool.self, forKey: .cut) ?? false
            json = try c.decodeIfPresent(Bool.self, forKey: .json) ?? false
        }
    }

    let ok: Bool
    /// What was seen, for a person: the value, or how much matched.
    let value: String?
    /// Never at a first look for a trigger that waits for a change or for something new.
    let wouldFire: Bool
    let summary: String?
    let error: String?
    let sample: Sample?

    enum CodingKeys: String, CodingKey {
        case ok, value, summary, error, sample
        case wouldFire = "would_fire"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? false
        // A string today; anything else is shown as its JSON.
        if let raw = try c.decodeIfPresent(JSONValue.self, forKey: .value), raw != .null {
            value = raw.prettyPrinted
        } else {
            value = nil
        }
        wouldFire = try c.decodeIfPresent(Bool.self, forKey: .wouldFire) ?? false
        summary = try c.decodeIfPresent(String.self, forKey: .summary)
        error = try c.decodeIfPresent(String.self, forKey: .error)
        sample = try c.decodeIfPresent(Sample.self, forKey: .sample)
    }
}

// MARK: Creating and changing agents

/// A field of a `PATCH` body that is left out (stays as it is), sent as `null` (back to the
/// default), or sent with a value.
enum Nullable<Value: Encodable & Sendable>: Sendable {
    case absent, null
    case value(Value)
}

/// The body of `POST /api/agents` and `PATCH /api/agents/{id}`. Only what is set is written
/// (the server refuses fields it doesn't know and leaves alone the ones it isn't sent); `tools`,
/// `notify` and the two limits can also be an explicit `null`.
struct AgentBody: Encodable, Sendable {
    var title: String?
    var prompt: String?
    /// A registered model's id; empty for cedar's default.
    var model: String?
    var reasoning: String?
    var enabled: Bool?
    /// The chat the agent is made from.
    var conversationId: String?
    /// Left out when creating: an agent that events start gets a careful selection, a clock
    /// agent the conversation's tools or the defaults. `null` is the server's defaults.
    var tools: Nullable<[String]> = .absent
    var notify: Nullable<String> = .absent
    /// Required when creating; in a `PATCH` it replaces the whole trigger, and what it had seen
    /// is forgotten.
    var trigger: Trigger?
    /// This device's IANA time zone, for run titles when the trigger has no zone of its own.
    var timezone: String?
    var minIntervalSecs: Nullable<Int> = .absent
    var maxRunsPerDay: Nullable<Int> = .absent

    enum CodingKeys: String, CodingKey {
        case title, prompt, model, reasoning, enabled, tools, notify, trigger, timezone
        case conversationId = "conversation_id"
        case minIntervalSecs = "min_interval_secs"
        case maxRunsPerDay = "max_runs_per_day"
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encodeIfPresent(title, forKey: .title)
        try c.encodeIfPresent(prompt, forKey: .prompt)
        try c.encodeIfPresent(model, forKey: .model)
        try c.encodeIfPresent(reasoning, forKey: .reasoning)
        try c.encodeIfPresent(enabled, forKey: .enabled)
        try c.encodeIfPresent(conversationId, forKey: .conversationId)
        try Self.put(tools, .tools, in: &c)
        try Self.put(notify, .notify, in: &c)
        try c.encodeIfPresent(trigger, forKey: .trigger)
        try c.encodeIfPresent(timezone, forKey: .timezone)
        try Self.put(minIntervalSecs, .minIntervalSecs, in: &c)
        try Self.put(maxRunsPerDay, .maxRunsPerDay, in: &c)
    }

    private static func put<T: Encodable & Sendable>(_ field: Nullable<T>, _ key: CodingKeys,
                                                     in c: inout KeyedEncodingContainer<CodingKeys>) throws {
        switch field {
        case .absent: break
        case .null: try c.encodeNil(forKey: key)
        case .value(let value): try c.encode(value, forKey: key)
        }
    }
}

/// The body of `POST /api/agents/test`.
struct TriggerTestBody: Encodable, Sendable {
    let trigger: Trigger
}

// MARK: What the agent form picks from

/// cedar's tools by group (`GET /api/tools`). A tool with `pollable` can be a condition's
/// source; the group `custom` holds personal and community tools.
struct ToolCatalog: Decodable, Sendable {
    struct Tool: Decodable, Identifiable, Hashable, Sendable {
        let name: String
        let description: String
        /// It changes something outside cedar (sends mail, creates an event).
        let writes: Bool
        /// It can be called outside a run, so a condition can watch its result.
        let pollable: Bool

        var id: String { name }

        enum CodingKeys: String, CodingKey {
            case name, description, writes, pollable
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            name = try c.decode(String.self, forKey: .name)
            description = try c.decodeIfPresent(String.self, forKey: .description) ?? ""
            writes = try c.decodeIfPresent(Bool.self, forKey: .writes) ?? false
            pollable = try c.decodeIfPresent(Bool.self, forKey: .pollable) ?? false
        }
    }

    struct Group: Decodable, Identifiable, Hashable, Sendable {
        let id: String
        let label: String
        let available: Bool
        let tools: [Tool]
        /// `false` for an MCP server whose tools aren't known until a run has connected to it.
        let discovered: Bool?

        /// Personal and community tools: code that runs in a container of its own.
        var isPersonal: Bool { id == ToolCatalog.personalGroup }

        enum CodingKeys: String, CodingKey {
            case id, label, available, tools, discovered
        }

        init(from decoder: Decoder) throws {
            let c = try decoder.container(keyedBy: CodingKeys.self)
            id = try c.decode(String.self, forKey: .id)
            label = try c.decodeIfPresent(String.self, forKey: .label) ?? id
            available = try c.decodeIfPresent(Bool.self, forKey: .available) ?? false
            tools = try c.decodeIfPresent([Tool].self, forKey: .tools) ?? []
            discovered = try c.decodeIfPresent(Bool.self, forKey: .discovered)
        }
    }

    static let personalGroup = "custom"
    static let empty = ToolCatalog(groups: [])

    let groups: [Group]

    enum CodingKeys: String, CodingKey {
        case groups
    }

    init(groups: [Group]) {
        self.groups = groups
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        groups = try c.decodeIfPresent([Group].self, forKey: .groups) ?? []
    }

    /// The tools a condition can watch.
    var pollableTools: [Tool] {
        groups.flatMap { $0.tools.filter(\.pollable) }
    }

    /// Whether `name` is a personal tool: its code runs unattended on every check.
    func isPersonal(tool name: String) -> Bool {
        groups.contains { $0.isPersonal && $0.tools.contains { $0.name == name } }
    }

    /// The selection the server makes for an agent that events start when nobody chose:
    /// everything except personal tools and the tools that write. Such a run reads text other
    /// people wrote, with nobody there to approve a call.
    var carefulTools: [String] {
        var out: [String] = []
        for group in groups where !group.isPersonal {
            if group.tools.contains(where: \.writes) {
                out.append(contentsOf: group.tools.filter { !$0.writes }.map(\.name))
            } else if group.available, group.discovered != false {
                out.append(group.id)
            }
        }
        return out.sorted()
    }
}

/// What the form needs of `GET /api/notifications`: the default method and the ones set up.
struct NotificationStatus: Decodable, Sendable {
    /// `none`, or a method.
    let defaultMethod: String
    /// The methods that can send right now, in the order pickers show them.
    let configured: [String]

    enum CodingKeys: String, CodingKey {
        case configured
        case defaultMethod = "default_method"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        defaultMethod = try c.decodeIfPresent(String.self, forKey: .defaultMethod) ?? "none"
        configured = try c.decodeIfPresent([String].self, forKey: .configured) ?? []
    }

    /// A method's name: the server's ids, as the web UI words them.
    static func label(_ method: String) -> String {
        switch method {
        case "none": return L10n.string("Only in the app")
        case "desktop": return L10n.string("Desktop app")
        case "browser": return L10n.string("Browser push")
        case "ios": return L10n.string("iPhone push")
        case "android": return L10n.string("Android push")
        case "pushover": return "Pushover"
        case "email": return L10n.string("Email")
        default: return method
        }
    }
}

/// `POST /api/conversations/{id}/compact`: the conversation as one prompt, for making an agent
/// from it.
struct Compacted: Decodable, Sendable {
    let prompt: String
}
