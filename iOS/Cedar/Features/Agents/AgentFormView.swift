import SwiftUI

/// What the agent form opens with: an agent to change, a conversation to make one from, or
/// nothing for a new one.
struct AgentFormContext: Identifiable {
    let id = UUID()
    /// The agent to change; `nil` for a new one.
    var agent: Agent?
    /// The finished conversation a new agent is made from: the server writes the prompt from it.
    var conversationId: String?
    /// A new agent's name, model and reasoning level to start from (the conversation's).
    var title = ""
    var model = ""
    var reasoning = ""
    /// The server's limits and defaults, as `GET /api/agents` told them.
    var limits = AgentList.Limits()
}

/// Makes or changes an agent: its name and prompt, what starts it (every kind of trigger, each
/// with its own fields), and the model, notification, limits and tools its runs get. A trigger
/// that watches something (a value, a record set, mail, a calendar, a Drive folder) can be tried
/// out before saving. Save sends only what the server needs
/// (`AgentDraft` builds the body); Cancel asks before dropping edits.
struct AgentFormView: View {
    let context: AgentFormContext
    /// Called with the agent as the server answered it, before the sheet goes.
    let onSaved: (Agent) -> Void
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var draft: AgentDraft
    /// The draft as it started, to tell whether there is anything to lose.
    @State private var initial: AgentDraft
    @State private var catalog = ToolCatalog.empty
    @State private var recordSets: [RecordSet] = []
    @State private var notifications: NotificationStatus?
    @State private var saving = false
    @State private var error: String?
    @State private var confirmingDiscard = false
    /// The server is writing the prompt from the conversation.
    @State private var compacting = false
    @State private var compactError: String?
    @State private var testing = false
    @State private var test: TriggerTest?
    @State private var testError: String?

    init(context: AgentFormContext, onSaved: @escaping (Agent) -> Void = { _ in }) {
        self.context = context
        self.onSaved = onSaved
        var draft: AgentDraft
        if let agent = context.agent {
            draft = AgentDraft(agent: agent, limits: context.limits)
        } else {
            draft = AgentDraft(limits: context.limits)
            draft.title = context.title
            draft.model = context.model
            draft.reasoning = context.reasoning
        }
        _draft = State(initialValue: draft)
        _initial = State(initialValue: draft)
    }

    private var isNew: Bool { context.agent == nil }

    var body: some View {
        NavigationStack {
            Form {
                basics
                trigger
                if draft.canTest { tryIt }
                if draft.byEvent { limits }
                tools
                model
                notify
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                            .foregroundStyle(.red)
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(Theme.canvas)
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(isNew ? "New Agent" : "Edit Agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        if hasChanges {
                            confirmingDiscard = true
                        } else {
                            dismiss()
                        }
                    }
                    .disabled(saving)
                }
                ToolbarItem(placement: .confirmationAction) {
                    if saving {
                        ProgressView()
                    } else {
                        Button("Save") { save() }
                            .disabled(!canSave)
                    }
                }
            }
            .confirmationDialog("Discard Changes?", isPresented: $confirmingDiscard, titleVisibility: .visible) {
                Button("Discard Changes", role: .destructive) { dismiss() }
                Button("Keep Editing", role: .cancel) {}
            } message: {
                Text(isNew ? "The agent is not saved yet." : "Your changes to the agent are not saved yet.")
            }
            .task { await load() }
            .onChange(of: draft.kind) {
                // Another kind of trigger: what the last one's check found no longer applies.
                test = nil
                testError = nil
                if !toolChoices.contains(draft.toolsChoice) { draft.toolsChoice = .automatic }
            }
            .onChange(of: draft.model) {
                // A level the new model doesn't offer is dropped.
                if let levels = selectedModel?.reasoningLevels, !draft.reasoning.isEmpty, !levels.contains(draft.reasoning) {
                    draft.reasoning = ""
                }
            }
        }
        .interactiveDismissDisabled(hasChanges || saving)
    }

    // MARK: Name and prompt

    private var basics: some View {
        Section {
            TextField("Name", text: $draft.title)
            if compacting {
                HStack(spacing: 10) {
                    ProgressView()
                    Text("Reading the conversation to write the prompt…")
                        .foregroundStyle(.secondary)
                }
            } else {
                TextField("What should the agent do each time it runs?", text: $draft.prompt, axis: .vertical)
                    .lineLimit(4...14)
            }
            if let compactError {
                Label(compactError, systemImage: "exclamationmark.triangle")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                Button("Try Again") {
                    Task { await compact() }
                }
            }
        } header: {
            Text("Agent")
        } footer: {
            Text(context.conversationId != nil && isNew
                 ? "Cedar wrote this prompt from the conversation. Read it and change what you like: every run starts fresh with it, in a conversation of its own."
                 : "Every run starts fresh with this prompt, in a conversation of its own.")
        }
    }

    // MARK: Trigger

    private var trigger: some View {
        Section {
            if let fixed = draft.fixedTrigger {
                Label(fixed.summary, systemImage: fixed.systemImage)
                Text("This trigger was made by a newer version of Cedar. It stays as it is; the rest of the agent can be changed here.")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
            } else {
                Picker("Starts", selection: $draft.kind) {
                    ForEach(AgentDraft.Kind.allCases) { kind in
                        Text(kind.label).tag(kind)
                    }
                }
                kindFields
                if draft.hasForeignTimeZone {
                    Button("Use This Device's Time Zone") { draft.useDeviceTimeZone() }
                }
            }
        } header: {
            Text("Trigger")
        } footer: {
            Text(triggerFooter)
        }
    }

    @ViewBuilder
    private var kindFields: some View {
        switch draft.kind {
        case .time:
            Picker("Repeats", selection: $draft.frequency) {
                Text(L10n.string("Daily")).tag("daily")
                Text(L10n.string("Weekdays")).tag("weekdays")
                Text(L10n.string("Weekly")).tag("weekly")
            }
            if draft.frequency == "weekly" {
                Picker("Day", selection: $draft.weekday) {
                    ForEach(0..<7, id: \.self) { day in
                        Text(verbatim: Self.weekdayName(day)).tag(day)
                    }
                }
            }
            DatePicker("Time", selection: $draft.clock, displayedComponents: .hourAndMinute)
                .environment(\.timeZone, zone)
        case .once:
            DatePicker("When", selection: $draft.onceAt, displayedComponents: [.date, .hourAndMinute])
                .environment(\.timeZone, zone)
        case .interval:
            EveryRow(title: "Every", every: $draft.interval)
        case .monthly:
            Picker("Day of the Month", selection: $draft.monthDay) {
                ForEach(1...31, id: \.self) { day in
                    Text(verbatim: "\(day)").tag(day)
                }
            }
            DatePicker("Time", selection: $draft.clock, displayedComponents: .hourAndMinute)
                .environment(\.timeZone, zone)
        case .condition:
            conditionFields
        case .webhook:
            Text("Nothing to fill in. Once the agent is saved it gets an address and a secret; a call to the address with the secret starts a run, and what the call sends goes to the run as data.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        case .email:
            TextField(text: $draft.query, prompt: Text(verbatim: "from:billing@example.com has:attachment")) {
                Text("Gmail search")
            }
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            EveryRow(title: "Check Every", every: $draft.check)
        case .calendar:
            TextField("Only events matching (optional)", text: $draft.calendarFilter)
                .autocorrectionDisabled()
            Toggle("Before an Event Starts", isOn: $draft.remindBefore)
            if draft.remindBefore {
                EveryRow(title: "How Long Before", every: $draft.before)
            }
            Toggle("When an Event Is Added", isOn: $draft.newEvents)
            EveryRow(title: "Check Every", every: $draft.check)
        case .records:
            recordSetPicker
            Picker("Watch For", selection: $draft.rowsOn) {
                Text("New rows").tag("added")
                Text("New or changed rows").tag("changed")
                // A choice from a newer server, kept until it is changed.
                if draft.rowsOn != "added", draft.rowsOn != "changed" {
                    Text(verbatim: draft.rowsOn).tag(draft.rowsOn)
                }
            }
            EveryRow(title: "Check Every", every: $draft.check)
        case .drive:
            TextField(text: $draft.driveFolder, prompt: Text(verbatim: "https://drive.google.com/drive/folders/…")) {
                Text("Folder address or ID")
            }
            .keyboardType(.URL)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            EveryRow(title: "Check Every", every: $draft.check)
        case .device:
            Text("Nothing to fill in. The agent waits until your device asks for a run: add the Run Cedar Agent action to a shortcut in the Shortcuts app and choose this agent. Text you pass to the action goes to the run as data.")
                .font(.footnote)
                .foregroundStyle(.secondary)
            Text("An automation in the Shortcuts app can run that shortcut for you, for example when you arrive at or leave a place, or when your iPhone connects to CarPlay.")
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    /// The record sets cedar keeps, for a condition on one and for a trigger on its rows.
    private var recordSetPicker: some View {
        Picker("Record Set", selection: $draft.recordSet) {
            Text("Choose…").tag("")
            ForEach(recordSets) { set in
                Text(verbatim: set.name).tag(set.name)
            }
            // The saved pick, kept until the list is loaded or it is changed.
            if !draft.recordSet.isEmpty, !recordSets.contains(where: { $0.name == draft.recordSet }) {
                Text(verbatim: draft.recordSet).tag(draft.recordSet)
            }
        }
    }

    /// What a condition watches, where the value is in it, and the test it has to pass.
    @ViewBuilder
    private var conditionFields: some View {
        Picker("Watch", selection: $draft.sourceType) {
            ForEach(AgentDraft.SourceType.allCases) { type in
                Text(type.label).tag(type)
            }
        }
        switch draft.sourceType {
        case .url:
            TextField(text: $draft.url, prompt: Text(verbatim: "https://example.com/status.json")) {
                Text("Web address")
            }
            .keyboardType(.URL)
            .textContentType(.URL)
            .textInputAutocapitalization(.never)
            .autocorrectionDisabled()
            Picker("Read As", selection: $draft.urlFormat) {
                Text(verbatim: "JSON").tag("json")
                Text("Plain text").tag("text")
            }
        case .tool:
            Picker("Tool", selection: $draft.toolName) {
                Text("Choose…").tag("")
                ForEach(catalog.pollableTools) { tool in
                    Text(verbatim: tool.name).tag(tool.name)
                }
                // The saved pick, kept until the list is loaded or it is changed.
                if !draft.toolName.isEmpty, !catalog.pollableTools.contains(where: { $0.name == draft.toolName }) {
                    Text(verbatim: draft.toolName).tag(draft.toolName)
                }
            }
            if let tool = catalog.pollableTools.first(where: { $0.name == draft.toolName }), !tool.description.isEmpty {
                Text(tool.description)
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(4)
            }
            VStack(alignment: .leading, spacing: 4) {
                Text("Input, as JSON")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                TextField(text: $draft.toolInput, prompt: Text(verbatim: "{}"), axis: .vertical) {
                    Text("Input, as JSON")
                }
                .font(.footnote.monospaced())
                .lineLimit(1...8)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            }
        case .records:
            recordSetPicker
        }
        TextField(text: $draft.extract, prompt: Text(verbatim: draft.sourceType == .records ? "count" : "price.eur")) {
            Text("Path to the value (optional)")
        }
        .font(.body.monospaced())
        .textInputAutocapitalization(.never)
        .autocorrectionDisabled()
        Picker("Run When the Value", selection: $draft.op) {
            ForEach(Trigger.Predicate.ops, id: \.self) { op in
                Text(Trigger.Predicate.label(op: op)).tag(op)
            }
            // An operator from a newer server, kept until it is changed.
            if !Trigger.Predicate.ops.contains(draft.op) {
                Text(verbatim: draft.op).tag(draft.op)
            }
        }
        if draft.op != "changes" {
            TextField(Trigger.Predicate.numberOps.contains(draft.op) ? "Number" : "Text, number, true or false", text: $draft.value)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
        }
        EveryRow(title: "Check Every", every: $draft.check)
    }

    /// The zone the time pickers show their times in.
    private var zone: TimeZone { TimeZone(identifier: draft.timezone) ?? .current }

    /// Monday for 0, in the interface language.
    private static func weekdayName(_ day: Int) -> String {
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = L10n.locale
        // The calendar's names start on Sunday.
        let names = calendar.standaloneWeekdaySymbols
        let index = (day + 1) % 7
        return names.indices.contains(index) ? names[index].capitalized(with: L10n.locale) : String(day)
    }

    /// What the section says under the fields: how the kind behaves, and the safety notes.
    private var triggerFooter: String {
        guard draft.fixedTrigger == nil else { return "" }
        var parts: [String] = []
        switch draft.kind {
        case .time:
            break
        case .once:
            parts.append(L10n.string("It runs once, then switches itself off."))
        case .interval:
            parts.append(L10n.format("No more often than every %@.", Trigger.duration(context.limits.minIntervalMinutes)))
        case .monthly:
            parts.append(L10n.string("In a shorter month it runs on the last day."))
        case .condition:
            parts.append(L10n.string("Cedar looks at the value on every check, with no AI model involved, and starts a run when it passes the test. A path like price.eur or /items/0/name picks one value out of a JSON result; without one, the whole result is the value. A test other than \u{201C}changes\u{201D} starts a run when it begins to hold, and again only after it stopped holding."))
            if draft.sourceType == .records {
                parts.append(L10n.string("A record set is seen as its count, when it was last updated and its records, so the path \u{201C}count\u{201D} watches for new rows."))
            }
            if draft.sourceType == .tool, catalog.isPersonal(tool: draft.toolName) {
                parts.append(L10n.format("This is a personal tool: its code runs unattended on every check, so it is checked every %@ at most.",
                                         Trigger.duration(context.limits.personalCheckMinutes)))
            }
        case .webhook:
            break
        case .email:
            parts.append(L10n.string("Gmail's search syntax; leave it empty for any new mail. Google has to be connected on the server."))
        case .calendar:
            parts.append(L10n.string("Runs before events start, when new ones appear, or both. Google has to be connected on the server."))
        case .records:
            parts.append(L10n.string("Cedar looks at the set on every check, with no AI model involved. The first check only learns what is there; after that, rows that are new (or new or changed, if you choose so) start a run, which is handed those rows."))
        case .drive:
            parts.append(L10n.string("Open the folder in Google Drive in a browser and copy its address, or enter the folder's ID. New files in the folder start a run; files in its subfolders don't count, and the first check only learns what is there. Google has to be connected on the server."))
        case .device:
            parts.append(L10n.string("Any agent that isn't paused can be started from the Shortcuts app this way, whatever else starts it. This kind is for one that nothing else starts."))
        }
        if draft.hasForeignTimeZone {
            parts.append(L10n.format("Times are in %@.", draft.timezone))
        }
        if draft.kind.byEvent {
            parts.append(L10n.string("An agent that events start reads text other people wrote (mail, a web page, a shared file, what a webhook or a shortcut sends) and runs with nobody there to approve what its tools do."))
        }
        return parts.joined(separator: "\n\n")
    }

    // MARK: Try It

    private var tryIt: some View {
        Section {
            Button {
                Task { await runTest() }
            } label: {
                HStack {
                    Label("Try It", systemImage: "play")
                    Spacer()
                    if testing { ProgressView() }
                }
            }
            .disabled(testing)
            if let testError {
                Label(testError, systemImage: "xmark.circle.fill")
                    .font(.footnote)
                    .foregroundStyle(Theme.clay)
            }
            if let test {
                if test.ok {
                    if let value = test.value, !value.isEmpty {
                        VStack(alignment: .leading, spacing: 3) {
                            Text("What It Sees Now")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                            Text(verbatim: value)
                                .font(.callout.monospaced())
                                .textSelection(.enabled)
                                .lineLimit(6)
                        }
                    }
                    Label(test.wouldFire ? "A run would start now." : "No run would start now.",
                          systemImage: test.wouldFire ? "bolt.fill" : "moon.zzz")
                        .font(.footnote)
                        .foregroundStyle(test.wouldFire ? Theme.green : Color.secondary)
                    if let summary = test.summary, !summary.isEmpty {
                        Text(verbatim: L10n.labeled(L10n.string("Started by"), summary))
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                } else {
                    Label(test.error ?? L10n.string("The check failed."), systemImage: "xmark.circle.fill")
                        .font(.footnote)
                        .foregroundStyle(Theme.clay)
                }
                if let sample = test.sample, !sample.text.isEmpty {
                    DisclosureGroup("What the Check Returned") {
                        Text(verbatim: sample.text)
                            .font(.caption.monospaced())
                            .textSelection(.enabled)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if sample.cut {
                            Text("Cut off here; the rest is not shown.")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }
            }
        } footer: {
            Text("Checks once, now, and keeps nothing. Look in what the check returned to find the path to the value. A trigger that waits for a change or for something new never starts a run at its first look.")
        }
    }

    // MARK: Limits, tools, model, notifications

    private var limits: some View {
        Section {
            Toggle("Its Own Limits", isOn: $draft.customLimits)
            if draft.customLimits {
                EveryRow(title: "At Most Once Every", every: $draft.minInterval)
                Stepper(value: $draft.maxRunsPerDay, in: 1...500) {
                    LabeledContent("Runs a Day at Most") {
                        Text(verbatim: "\(draft.maxRunsPerDay)")
                            .monospacedDigit()
                    }
                }
            }
        } header: {
            Text("Limits")
        } footer: {
            Text(verbatim: L10n.format("How often events may start a run, so a busy source can't start one after another. Without limits of its own: at most once every %@, and %lld runs in 24 hours.",
                                       Trigger.duration(max(1, context.limits.minIntervalSecs / 60)), context.limits.maxRunsPerDay))
        }
    }

    private var tools: some View {
        Section {
            Picker("Tools", selection: $draft.toolsChoice) {
                ForEach(toolChoices) { choice in
                    Text(toolsLabel(choice)).tag(choice)
                }
            }
            if draft.toolsChoice == .custom {
                NavigationLink {
                    ToolsPickerView(catalog: catalog, selection: $draft.customTools)
                } label: {
                    LabeledContent("Chosen") {
                        Text(verbatim: L10n.format(draft.customTools.count == 1 ? "%lld tool or group" : "%lld tools and groups",
                                                   draft.customTools.count))
                    }
                }
            }
        } header: {
            Text("Tools")
        } footer: {
            Text(toolsFooter)
        }
    }

    /// The choices that make sense for what is being made or changed.
    private var toolChoices: [AgentDraft.ToolsChoice] {
        if isNew {
            // For a clock agent without a conversation, left to the server is the defaults.
            return draft.byEvent || context.conversationId != nil ? [.automatic, .all, .custom] : [.automatic, .custom]
        }
        return draft.byEvent && !catalog.groups.isEmpty ? [.automatic, .careful, .all, .custom] : [.automatic, .all, .custom]
    }

    private func toolsLabel(_ choice: AgentDraft.ToolsChoice) -> String {
        switch choice {
        case .automatic:
            if !isNew { return L10n.string("Keep as they are") }
            if draft.byEvent { return L10n.string("Careful selection") }
            return context.conversationId != nil ? L10n.string("Same as the conversation") : L10n.string("Server defaults")
        case .careful: return L10n.string("Careful selection")
        case .all: return L10n.string("Server defaults")
        case .custom: return L10n.string("Choose…")
        }
    }

    private var toolsFooter: String {
        guard draft.byEvent else {
            return L10n.string("The tools its runs may use.")
        }
        var text = L10n.string("An agent that events start gets a careful selection unless you choose otherwise: no personal tools, and none that write to your Google account.")
        if !isNew, draft.toolsChoice == .automatic, context.agent?.trigger.byEvent == false {
            text += "\n\n" + L10n.string("This agent keeps the tools it has now. Choose the careful selection unless you are sure about them.")
        }
        return text
    }

    private var model: some View {
        Section {
            Picker("AI Model", selection: $draft.model) {
                Text("Server default").tag("")
                ForEach(app.models) { model in
                    Text(model.label).tag(model.id)
                }
                // The saved pick, kept until the list is loaded or it is changed.
                if !draft.model.isEmpty, !app.models.contains(where: { $0.id == draft.model }) {
                    Text(verbatim: draft.model).tag(draft.model)
                }
            }
            if let levels = selectedModel?.reasoningLevels, !levels.isEmpty {
                Picker("Reasoning Effort", selection: $draft.reasoning) {
                    Text("Default").tag("")
                    ForEach(levels, id: \.self) { level in
                        Text(level.capitalized).tag(level)
                    }
                }
            }
        } header: {
            Text("AI Model")
        }
    }

    /// The picked model, or the server's default one when none is picked.
    private var selectedModel: ModelInfo? {
        draft.model.isEmpty ? app.models.first(where: \.isDefault) : app.models.first { $0.id == draft.model }
    }

    private var notify: some View {
        let configured = notifications?.configured ?? []
        return Section {
            Picker("Notify Me By", selection: $draft.notify) {
                Text(defaultMethodLabel).tag("")
                Text(NotificationStatus.label("none")).tag("none")
                ForEach(configured, id: \.self) { method in
                    Text(NotificationStatus.label(method)).tag(method)
                }
                // The saved pick, though it is no longer set up.
                if !draft.notify.isEmpty, draft.notify != "none", !configured.contains(draft.notify) {
                    Text(NotificationStatus.label(draft.notify)).tag(draft.notify)
                }
            }
        } header: {
            Text("Notifications")
        } footer: {
            Text("How Cedar tells you that a run has something for you. The methods are set up under Settings → Notifications in the web UI.")
        }
    }

    private var defaultMethodLabel: String {
        guard let notifications else { return L10n.string("Default") }
        return L10n.format("Default (%@)", NotificationStatus.label(notifications.defaultMethod))
    }

    // MARK: Actions

    private var hasChanges: Bool { draft != initial }

    private var canSave: Bool {
        !compacting
            && !draft.title.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && !draft.prompt.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && (isNew || hasChanges)
    }

    /// Reads what the pickers offer, and has the prompt written when the agent is made from a
    /// conversation. A list that can't be read leaves its picker with the saved pick only.
    private func load() async {
        guard let client = app.client else { return }
        async let tools = try? client.tools()
        async let records = try? client.records()
        async let status = try? client.notificationStatus()
        if isNew, context.conversationId != nil, draft.prompt.isEmpty {
            await compact()
        }
        if let loaded = await tools { catalog = loaded }
        recordSets = (await records)?.sets ?? []
        notifications = await status
    }

    /// Asks the server to turn the conversation into one prompt. The form can be filled in by
    /// hand when that fails.
    private func compact() async {
        guard let client = app.client, let conversationId = context.conversationId else { return }
        compacting = true
        compactError = nil
        defer { compacting = false }
        do {
            let prompt = try await client.compact(conversationId: conversationId).prompt
            if draft.prompt.isEmpty {
                draft.prompt = prompt
                // What the server wrote is where the form starts, not an edit to lose.
                initial.prompt = prompt
            }
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            compactError = error.localizedDescription
        }
    }

    private func runTest() async {
        testError = nil
        let trigger: Trigger
        do {
            trigger = try draft.trigger(limits: context.limits, personalTool: catalog.isPersonal(tool: draft.toolName))
        } catch {
            test = nil
            testError = error.localizedDescription
            return
        }
        testing = true
        defer { testing = false }
        do {
            test = try await app.testTrigger(trigger)
        } catch is CancellationError {
        } catch {
            test = nil
            testError = error.localizedDescription
        }
    }

    private func save() {
        error = nil
        let body: AgentBody
        do {
            if let agent = context.agent {
                body = try draft.updateBody(for: agent, limits: context.limits, catalog: catalog)
            } else {
                body = try draft.createBody(conversationId: context.conversationId, limits: context.limits, catalog: catalog)
            }
        } catch {
            self.error = error.localizedDescription
            return
        }
        saving = true
        Task {
            defer { saving = false }
            do {
                let saved: Agent
                if let agent = context.agent {
                    saved = try await app.updateAgent(id: agent.id, body)
                } else {
                    saved = try await app.createAgent(body)
                }
                onSaved(saved)
                dismiss()
            } catch {
                // The server's reasons (a tool that can't be watched, a method not set up) as it gives them.
                self.error = error.localizedDescription
            }
        }
    }
}

/// A length of time in a form row: a number and its unit.
private struct EveryRow: View {
    let title: LocalizedStringKey
    @Binding var every: AgentDraft.Every

    var body: some View {
        HStack(spacing: 8) {
            Text(title)
            Spacer(minLength: 8)
            TextField(value: $every.amount, format: .number) {
                Text(title)
            }
            .keyboardType(.numberPad)
            .multilineTextAlignment(.trailing)
            .frame(width: 64)
            Picker("Unit", selection: $every.unit) {
                ForEach(AgentDraft.Unit.allCases) { unit in
                    Text(unit.label).tag(unit)
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
        }
    }
}

/// Picks an agent's tools from cedar's catalog: everything, whole groups, or single tools of a
/// group that isn't picked as a whole.
struct ToolsPickerView: View {
    let catalog: ToolCatalog
    @Binding var selection: Set<String>

    var body: some View {
        List {
            if catalog.groups.isEmpty {
                Section {
                    Label("The server's tools could not be loaded.", systemImage: "exclamationmark.triangle")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
            }
            Section {
                Toggle("Everything", isOn: picked("*"))
            } footer: {
                Text("Every tool the server offers, now and later.")
            }
            if !selection.contains("*") {
                ForEach(catalog.groups) { group in
                    Section {
                        Toggle(isOn: picked(group.id)) {
                            Text(verbatim: L10n.format("All of %@", group.label))
                        }
                        if !selection.contains(group.id) {
                            ForEach(group.tools) { tool in
                                Toggle(isOn: picked(tool.name)) {
                                    VStack(alignment: .leading, spacing: 2) {
                                        HStack(spacing: 6) {
                                            Text(verbatim: tool.name)
                                                .font(.callout.monospaced())
                                            if tool.writes {
                                                Text("Writes")
                                                    .font(.caption2)
                                                    .padding(.horizontal, 6)
                                                    .padding(.vertical, 1)
                                                    .background(Theme.raised, in: Capsule())
                                            }
                                        }
                                        if !tool.description.isEmpty {
                                            Text(tool.description)
                                                .font(.caption)
                                                .foregroundStyle(.secondary)
                                                .lineLimit(2)
                                        }
                                    }
                                }
                            }
                        }
                    } header: {
                        Text(group.label)
                    } footer: {
                        if group.isPersonal {
                            Text("Personal and community tools run their own code.")
                        } else if !group.available {
                            Text("Not set up on the server right now.")
                        }
                    }
                }
                if !unlisted.isEmpty {
                    Section {
                        ForEach(unlisted, id: \.self) { name in
                            Toggle(isOn: picked(name)) {
                                Text(verbatim: name).font(.callout.monospaced())
                            }
                        }
                    } header: {
                        Text("Other")
                    }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Theme.canvas)
        .navigationTitle("Tools")
        .navigationBarTitleDisplayMode(.inline)
    }

    /// Picked names the catalog doesn't list (an MCP server's tools, or ones since removed), so
    /// they can be switched off.
    private var unlisted: [String] {
        var known: Set<String> = ["*"]
        for group in catalog.groups {
            known.insert(group.id)
            known.formUnion(group.tools.map(\.name))
        }
        return selection.filter { !known.contains($0) }.sorted()
    }

    private func picked(_ name: String) -> Binding<Bool> {
        Binding(
            get: { selection.contains(name) },
            set: { on in
                if on {
                    selection.insert(name)
                } else {
                    selection.remove(name)
                }
            }
        )
    }
}
