import Foundation

/// What the agent form holds while it is filled in: every trigger kind's fields side by side, so
/// switching the kind keeps what was typed. `createBody` and `updateBody` turn it into what the
/// server takes, or say what is missing.
struct AgentDraft: Equatable, Sendable {
    /// The trigger kinds the form can make.
    enum Kind: String, CaseIterable, Identifiable, Sendable {
        case time, once, interval, monthly, condition, webhook, email, calendar

        var id: String { rawValue }

        var label: String {
            switch self {
            case .time: return L10n.string("Daily or weekly")
            case .once: return L10n.string("Once")
            case .interval: return L10n.string("Every so often")
            case .monthly: return L10n.string("Monthly")
            case .condition: return L10n.string("When something changes")
            case .webhook: return L10n.string("When a webhook is called")
            case .email: return L10n.string("When mail arrives")
            case .calendar: return L10n.string("Around calendar events")
            }
        }

        var systemImage: String { Trigger.systemImage(forKind: rawValue) }
        /// The server checks something for it between runs, so it can be tried out.
        var watches: Bool { Trigger.watches(kind: rawValue) }
        /// Something outside starts its runs: these get the limits and the careful tools.
        var byEvent: Bool { watches || self == .webhook }
    }

    enum Unit: String, CaseIterable, Identifiable, Sendable {
        case minutes, hours, days

        var id: String { rawValue }

        /// How many minutes one of it is.
        var length: Int {
            switch self {
            case .minutes: return 1
            case .hours: return 60
            case .days: return 1440
            }
        }

        var label: String {
            switch self {
            case .minutes: return L10n.string("minutes")
            case .hours: return L10n.string("hours")
            case .days: return L10n.string("days")
            }
        }
    }

    /// A length of time as the form asks for it: a number and a unit.
    struct Every: Equatable, Sendable {
        var amount: Int
        var unit: Unit

        var minutes: Int { amount * unit.length }

        init(amount: Int, unit: Unit) {
            self.amount = amount
            self.unit = unit
        }

        /// The largest unit that divides it: 120 minutes is 2 hours.
        init(minutes: Int) {
            if minutes > 0, minutes % 1440 == 0 {
                self.init(amount: minutes / 1440, unit: .days)
            } else if minutes > 0, minutes % 60 == 0 {
                self.init(amount: minutes / 60, unit: .hours)
            } else {
                self.init(amount: max(0, minutes), unit: .minutes)
            }
        }
    }

    enum SourceType: String, CaseIterable, Identifiable, Sendable {
        case url, tool, records

        var id: String { rawValue }

        var label: String {
            switch self {
            case .url: return L10n.string("Web address")
            case .tool: return L10n.string("Tool")
            case .records: return L10n.string("Record set")
            }
        }
    }

    /// Which tools the agent's runs get.
    enum ToolsChoice: String, Identifiable, Sendable {
        /// Left to the server when creating (a careful selection for an agent that events
        /// start, else the conversation's tools or the defaults); unchanged when editing.
        case automatic
        /// Everything except personal tools and the tools that write, as a list.
        case careful
        /// The server's default tools (`null`).
        case all
        /// The groups and tools picked in `customTools`.
        case custom

        var id: String { rawValue }
    }

    /// Why the form can't be saved, or the trigger tried out, yet.
    enum Problem: LocalizedError, Equatable {
        case titleMissing, promptMissing, urlInvalid, toolMissing, toolInputInvalid, recordSetMissing
        case numberNeeded, valueNeeded, amountMissing, tooLong, calendarNeedsOne, oncePast, toolsUnavailable, noTools, notEditable
        case intervalTooShort(Int)
        case checkTooOften(Int)

        var errorDescription: String? {
            switch self {
            case .titleMissing: return L10n.string("Give the agent a name.")
            case .promptMissing: return L10n.string("Write what the agent should do.")
            case .urlInvalid: return L10n.string("The address must start with https:// or http://.")
            case .toolMissing: return L10n.string("Choose the tool to watch.")
            case .toolInputInvalid: return L10n.string("The tool's input must be a JSON object, like {\"city\": \"Zurich\"}.")
            case .recordSetMissing: return L10n.string("Choose the record set to watch.")
            case .numberNeeded: return L10n.string("Enter a number to compare with.")
            case .valueNeeded: return L10n.string("Enter the value to compare with.")
            case .amountMissing: return L10n.string("Enter how often: a number of at least 1.")
            case .tooLong: return L10n.string("That is longer than a week.")
            case .calendarNeedsOne: return L10n.string("Choose a time before events, new events, or both.")
            case .oncePast: return L10n.string("Pick a time in the future.")
            case .toolsUnavailable: return L10n.string("The tools couldn't be loaded, so a careful selection can't be made. Try again.")
            case .noTools: return L10n.string("Choose at least one tool.")
            case .notEditable: return L10n.string("This version of the app can't change this trigger.")
            case .intervalTooShort(let minutes): return L10n.format("An interval can't be shorter than %lld minutes.", minutes)
            case .checkTooOften(let minutes): return L10n.format("A personal tool can be checked every %lld minutes at most.", minutes)
            }
        }
    }

    var title = ""
    var prompt = ""
    /// A registered model's id; empty for the server's default.
    var model = ""
    var reasoning = ""
    /// `none`, a method, or empty for the default one.
    var notify = ""
    var kind: Kind = .time
    /// The trigger of the agent being edited when this version can't edit it (a kind it doesn't
    /// know). It is shown as it is and never sent.
    var fixedTrigger: Trigger?
    /// The zone the times below are in: the trigger's when editing, else this device's.
    var timezone = TimeZone.current.identifier

    // Daily or weekly, monthly
    var frequency = "daily"
    /// 0–6, Monday–Sunday.
    var weekday = 0
    /// The time of day; only its hour and minute in `timezone` count.
    var clock: Date
    var monthDay = 1
    // Once
    var onceAt: Date
    // Every so often
    var interval = Every(amount: 1, unit: .hours)

    // When something changes
    var sourceType: SourceType = .url
    var url = ""
    /// `json` or `text`.
    var urlFormat = "json"
    var toolName = ""
    /// The tool's input, as JSON text.
    var toolInput = "{}"
    var recordSet = ""
    /// Where in a JSON result the value is: `/price/eur` or `price.eur`.
    var extract = ""
    var op = "changes"
    var value = ""
    /// How often a watching trigger checks.
    var check = Every(amount: 5, unit: .minutes)

    // Mail
    var query = ""

    // Calendar
    /// Kept as the agent has it; the form has no field for it.
    var calendarId: String?
    var calendarFilter = ""
    var remindBefore = true
    var before = Every(amount: 15, unit: .minutes)
    var newEvents = false

    // Limits, for the kinds that events start
    var customLimits = false
    var minInterval = Every(amount: 5, unit: .minutes)
    var maxRunsPerDay = 24

    var toolsChoice: ToolsChoice = .automatic
    var customTools: Set<String> = []

    /// A new agent: daily at nine, with the server's defaults.
    init(limits: AgentList.Limits = AgentList.Limits(), now: Date = Date()) {
        let zone = TimeZone.current.identifier
        let nine = AgentClock.date(time: "09:00", timezone: zone, on: now) ?? now
        clock = nine
        // Tomorrow at nine.
        onceAt = nine.addingTimeInterval(24 * 3600)
        check = Every(minutes: limits.checkMinutes)
        minInterval = Every(minutes: max(1, limits.minIntervalSecs / 60))
        maxRunsPerDay = limits.maxRunsPerDay
    }

    /// The form for an agent as the server has it.
    init(agent: Agent, limits: AgentList.Limits = AgentList.Limits(), now: Date = Date()) {
        self.init(limits: limits, now: now)
        title = agent.title
        prompt = agent.prompt
        model = agent.model ?? ""
        reasoning = agent.reasoning ?? ""
        notify = agent.notify ?? ""
        customTools = Set(agent.tools ?? [])
        if agent.minIntervalSecs != nil || agent.maxRunsPerDay != nil {
            customLimits = true
            if let seconds = agent.minIntervalSecs { minInterval = Every(minutes: max(1, (seconds + 59) / 60)) }
            if let runs = agent.maxRunsPerDay { maxRunsPerDay = runs }
        }
        switch agent.trigger {
        case .time(let frequency, let weekday, let time, let timezone):
            kind = .time
            self.frequency = frequency
            self.weekday = weekday ?? 0
            self.timezone = timezone
            clock = AgentClock.date(time: time, timezone: timezone, on: now) ?? clock
        case .once(let at, let timezone):
            kind = .once
            self.timezone = timezone
            onceAt = AgentClock.date(at: at, timezone: timezone) ?? onceAt
        case .interval(let everyMinutes):
            kind = .interval
            interval = Every(minutes: everyMinutes)
        case .monthly(let day, let time, let timezone):
            kind = .monthly
            monthDay = day
            self.timezone = timezone
            clock = AgentClock.date(time: time, timezone: timezone, on: now) ?? clock
        case .condition(let source, let extract, let predicate, let everyMinutes):
            kind = .condition
            switch source {
            case .url(let url, let format):
                sourceType = .url
                self.url = url
                urlFormat = format ?? "json"
            case .tool(let name, let input):
                sourceType = .tool
                toolName = name
                toolInput = input.prettyPrinted
            case .records(let set):
                sourceType = .records
                recordSet = set
            }
            self.extract = extract ?? ""
            op = predicate.op
            value = predicate.valueText
            check = Every(minutes: everyMinutes)
        case .webhook:
            kind = .webhook
        case .email(let query, let everyMinutes):
            kind = .email
            self.query = query
            check = Every(minutes: everyMinutes)
        case .calendar(let calendarId, let filter, let minutesBefore, let newEvents, let everyMinutes):
            kind = .calendar
            self.calendarId = calendarId
            calendarFilter = filter ?? ""
            remindBefore = minutesBefore != nil
            if let minutesBefore { before = Every(minutes: minutesBefore) }
            self.newEvents = newEvents
            check = Every(minutes: everyMinutes)
        case .unknown:
            fixedTrigger = agent.trigger
        }
    }

    /// The times are in a zone that isn't this device's (an agent made elsewhere).
    var hasForeignTimeZone: Bool {
        guard fixedTrigger == nil, timezone != TimeZone.current.identifier else { return false }
        return kind == .time || kind == .once || kind == .monthly
    }

    /// Moves the times to this device's zone, keeping what the clock says: 07:30 stays 07:30.
    mutating func useDeviceTimeZone() {
        let device = TimeZone.current.identifier
        guard timezone != device else { return }
        clock = AgentClock.date(time: AgentClock.time(clock, timezone: timezone), timezone: device, on: clock) ?? clock
        onceAt = AgentClock.date(at: AgentClock.at(onceAt, timezone: timezone), timezone: device) ?? onceAt
        timezone = device
    }

    /// Whether the trigger can be tried out (`POST /api/agents/test`): only one that watches.
    var canTest: Bool { fixedTrigger == nil && kind.watches }

    /// Whether events start the runs, whatever the form can or can't edit.
    var byEvent: Bool { fixedTrigger?.byEvent ?? kind.byEvent }

    // MARK: To the server

    /// The trigger as filled in, or what is missing. `personalTool` says the tool source is a
    /// personal tool, which is checked less often.
    func trigger(limits: AgentList.Limits = AgentList.Limits(), personalTool: Bool = false, now: Date = Date()) throws -> Trigger {
        if fixedTrigger != nil { throw Problem.notEditable }
        switch kind {
        case .time:
            return .time(frequency: frequency, weekday: frequency == "weekly" ? weekday : nil,
                         time: AgentClock.time(clock, timezone: timezone), timezone: timezone)
        case .once:
            guard onceAt > now else { throw Problem.oncePast }
            return .once(at: AgentClock.at(onceAt, timezone: timezone), timezone: timezone)
        case .interval:
            let minutes = interval.minutes
            guard minutes >= 1 else { throw Problem.amountMissing }
            guard minutes >= limits.minIntervalMinutes else { throw Problem.intervalTooShort(limits.minIntervalMinutes) }
            guard minutes <= Trigger.maxEveryMinutes else { throw Problem.tooLong }
            return .interval(everyMinutes: minutes)
        case .monthly:
            return .monthly(day: min(31, max(1, monthDay)), time: AgentClock.time(clock, timezone: timezone), timezone: timezone)
        case .condition:
            let watched = try source()
            let path = extract.trimmingCharacters(in: .whitespacesAndNewlines)
            let least = sourceType == .tool && personalTool ? limits.personalCheckMinutes : 1
            return .condition(source: watched, extract: path.isEmpty ? nil : path, predicate: try predicate(),
                              everyMinutes: try checkMinutes(atLeast: least))
        case .webhook:
            return .webhook
        case .email:
            return .email(query: query.trimmingCharacters(in: .whitespacesAndNewlines), everyMinutes: try checkMinutes(atLeast: 1))
        case .calendar:
            guard remindBefore || newEvents else { throw Problem.calendarNeedsOne }
            if remindBefore, before.minutes > Trigger.maxEveryMinutes { throw Problem.tooLong }
            let filter = calendarFilter.trimmingCharacters(in: .whitespacesAndNewlines)
            return .calendar(calendarId: calendarId, filter: filter.isEmpty ? nil : filter,
                             minutesBefore: remindBefore ? max(0, before.minutes) : nil, newEvents: newEvents,
                             everyMinutes: try checkMinutes(atLeast: 1))
        }
    }

    private func checkMinutes(atLeast least: Int) throws -> Int {
        let minutes = check.minutes
        guard minutes >= 1 else { throw Problem.amountMissing }
        guard minutes >= least else { throw Problem.checkTooOften(least) }
        guard minutes <= Trigger.maxEveryMinutes else { throw Problem.tooLong }
        return minutes
    }

    private func source() throws -> Trigger.Source {
        switch sourceType {
        case .url:
            let address = url.trimmingCharacters(in: .whitespacesAndNewlines)
            guard address.hasPrefix("https://") || address.hasPrefix("http://") else { throw Problem.urlInvalid }
            return .url(url: address, format: urlFormat)
        case .tool:
            let name = toolName.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !name.isEmpty else { throw Problem.toolMissing }
            // The keyboard's smart quotes aren't JSON's.
            let text = toolInput
                .replacingOccurrences(of: "[\u{201C}\u{201D}\u{201E}]", with: "\"", options: .regularExpression)
                .replacingOccurrences(of: "[\u{2018}\u{2019}]", with: "'", options: .regularExpression)
                .trimmingCharacters(in: .whitespacesAndNewlines)
            if text.isEmpty { return .tool(name: name, input: .object([:])) }
            guard let input = try? JSONDecoder().decode(JSONValue.self, from: Data(text.utf8)), case .object = input else {
                throw Problem.toolInputInvalid
            }
            return .tool(name: name, input: input)
        case .records:
            let set = recordSet.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !set.isEmpty else { throw Problem.recordSetMissing }
            return .records(set: set)
        }
    }

    private func predicate() throws -> Trigger.Predicate {
        if op == "changes" { return Trigger.Predicate(op: op, value: nil) }
        let text = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if Trigger.Predicate.numberOps.contains(op) {
            // A decimal comma is a decimal point.
            guard let number = Double(text.replacingOccurrences(of: ",", with: ".")), number.isFinite else {
                throw Problem.numberNeeded
            }
            return Trigger.Predicate(op: op, value: .number(number))
        }
        guard !text.isEmpty else { throw Problem.valueNeeded }
        return Trigger.Predicate(op: op, value: .string(text))
    }

    /// The body of `POST /api/agents`. `conversationId` is the chat the agent is made from.
    func createBody(conversationId: String? = nil, limits: AgentList.Limits = AgentList.Limits(),
                    catalog: ToolCatalog = .empty, now: Date = Date()) throws -> AgentBody {
        var body = try shared()
        body.conversationId = conversationId
        body.trigger = try trigger(limits: limits, personalTool: catalog.isPersonal(tool: toolName), now: now)
        body.tools = try tools(catalog: catalog)
        if kind.byEvent, customLimits {
            body.minIntervalSecs = .value(try limitSeconds())
            body.maxRunsPerDay = .value(maxRunsPerDay)
        }
        return body
    }

    /// The body of `PATCH /api/agents/{id}`. The trigger goes only when the form changed it:
    /// sending one makes the server forget what the old one had seen.
    func updateBody(for agent: Agent, limits: AgentList.Limits = AgentList.Limits(),
                    catalog: ToolCatalog = .empty, now: Date = Date()) throws -> AgentBody {
        var body = try shared()
        if fixedTrigger == nil, !sameTrigger(as: AgentDraft(agent: agent, limits: limits, now: now), limits: limits) {
            body.trigger = try trigger(limits: limits, personalTool: catalog.isPersonal(tool: toolName), now: now)
        }
        body.tools = try tools(catalog: catalog)
        if byEvent {
            if customLimits {
                body.minIntervalSecs = .value(try limitSeconds())
                body.maxRunsPerDay = .value(maxRunsPerDay)
            } else {
                // Back to the server's defaults, where the agent had its own.
                if agent.minIntervalSecs != nil { body.minIntervalSecs = .null }
                if agent.maxRunsPerDay != nil { body.maxRunsPerDay = .null }
            }
        }
        return body
    }

    /// What creating and changing both send. The model and the reasoning level always go, empty
    /// for the default, and the notification method as `null` for the default one.
    private func shared() throws -> AgentBody {
        let name = title.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw Problem.titleMissing }
        let text = prompt.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { throw Problem.promptMissing }
        var body = AgentBody()
        body.title = name
        body.prompt = text
        body.model = model
        body.reasoning = reasoning
        body.notify = notify.isEmpty ? .null : .value(notify)
        body.timezone = TimeZone.current.identifier
        return body
    }

    private func tools(catalog: ToolCatalog) throws -> Nullable<[String]> {
        switch toolsChoice {
        case .automatic: return .absent
        case .all: return .null
        case .careful:
            guard !catalog.groups.isEmpty else { throw Problem.toolsUnavailable }
            return .value(catalog.carefulTools)
        case .custom:
            guard !customTools.isEmpty else { throw Problem.noTools }
            return .value(customTools.sorted())
        }
    }

    private func limitSeconds() throws -> Int {
        let minutes = minInterval.minutes
        guard minutes >= 1 else { throw Problem.amountMissing }
        guard minutes <= Trigger.maxEveryMinutes else { throw Problem.tooLong }
        return minutes * 60
    }

    /// Whether the trigger is as in `other`, whatever else differs: both make the same trigger
    /// (a one-off's time may have passed), or the trigger's fields are the same.
    private func sameTrigger(as other: AgentDraft, limits: AgentList.Limits) -> Bool {
        if let mine = try? trigger(limits: limits, now: .distantPast),
           let theirs = try? other.trigger(limits: limits, now: .distantPast) {
            return mine == theirs
        }
        var mine = self
        mine.title = other.title
        mine.prompt = other.prompt
        mine.model = other.model
        mine.reasoning = other.reasoning
        mine.notify = other.notify
        mine.customLimits = other.customLimits
        mine.minInterval = other.minInterval
        mine.maxRunsPerDay = other.maxRunsPerDay
        mine.toolsChoice = other.toolsChoice
        mine.customTools = other.customTools
        return mine == other
    }
}
