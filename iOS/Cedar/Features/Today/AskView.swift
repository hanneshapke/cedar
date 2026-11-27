import SwiftUI

/// Asking with a When, in a sheet: one sentence, and when it should run. Now starts a
/// conversation (off the record, if chosen: a private chat). Later, Every… and When something
/// changes make an agent of the sentence, with the trigger the choices describe; anything a
/// trigger needs beyond that is in the agent form.
struct AskView: View {
    @Environment(AppModel.self) private var app
    @Environment(\.dismiss) private var dismiss
    @State private var draft = AskDraft()
    @State private var busy = false
    @State private var error: String?
    @FocusState private var typing: Bool

    /// Monday first, as the server counts weekdays.
    private static let weekdays = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays"]

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    TextField(L10n.string("Ask for something. Look it up, work through files, write it, keep watch."),
                              text: $draft.text, axis: .vertical)
                        .lineLimit(3...8)
                        .focused($typing)
                }
                Section {
                    Picker("When", selection: $draft.when) {
                        ForEach(AskWhen.allCases) { when in
                            Text(verbatim: when.title).tag(when)
                        }
                    }
                    .pickerStyle(.inline)
                    .labelsHidden()
                } header: {
                    Text("When")
                } footer: {
                    Text(verbatim: draft.when == .now && draft.offTheRecord
                         ? L10n.string("Off the record: this ask leaves no trace, nothing is remembered, and it is gone when you close it.")
                         : draft.when.hint)
                }
                details
                if let error {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle")
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .textSelection(.enabled)
                    }
                }
            }
            .navigationTitle("New Ask")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if busy {
                        ProgressView()
                    } else {
                        Button(draft.when.sendTitle) { send() }
                            .disabled(draft.content.isEmpty)
                    }
                }
            }
            .onAppear { typing = true }
            .onChange(of: draft.when) { _, when in
                // Nothing of an ask off the record is kept for later.
                if when != .now { draft.offTheRecord = false }
                error = nil
            }
        }
    }

    /// What the chosen When needs.
    @ViewBuilder
    private var details: some View {
        switch draft.when {
        case .now:
            if app.serverHasPrivateChats {
                Section {
                    Toggle("Off the record", isOn: $draft.offTheRecord)
                }
            }
        case .later:
            Section {
                DatePicker("At", selection: $draft.laterAt, in: Date()..., displayedComponents: [.date, .hourAndMinute])
            }
        case .every:
            Section {
                Picker("How often", selection: $draft.frequency) {
                    ForEach(AskFrequency.allCases) { frequency in
                        Text(verbatim: frequency.title).tag(frequency)
                    }
                }
                if draft.frequency == .weekly {
                    Picker("Weekday", selection: $draft.weekday) {
                        ForEach(Self.weekdays.indices, id: \.self) { index in
                            Text(verbatim: L10n.string(Self.weekdays[index])).tag(index)
                        }
                    }
                }
                DatePicker("Time", selection: $draft.time, displayedComponents: .hourAndMinute)
            }
        case .watch:
            Section {
                Picker("What to watch", selection: $draft.watch) {
                    ForEach(AskWatch.allCases) { watch in
                        Text(verbatim: watch.title).tag(watch)
                    }
                }
                .pickerStyle(.inline)
                .labelsHidden()
                switch draft.watch {
                case .email:
                    TextField(L10n.string("Gmail search"), text: $draft.query)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                case .calendar:
                    Stepper(value: $draft.minutesBefore, in: 5...1440, step: 5) {
                        Text(verbatim: L10n.format("%lld minutes before", draft.minutesBefore))
                    }
                case .device:
                    EmptyView()
                }
            } footer: {
                Text("For anything else to watch (a price, a page, your records), make an agent under Agents.")
            }
        }
    }

    private func send() {
        if let problem = draft.problem() {
            error = problem
            return
        }
        busy = true
        error = nil
        Task {
            defer { busy = false }
            do {
                try await app.ask(draft)
                dismiss()
            } catch is CancellationError {
            } catch {
                // The server's reasons (too many agents, Google not connected) as it gives them.
                self.error = error.localizedDescription
            }
        }
    }
}
