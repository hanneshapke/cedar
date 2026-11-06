import SwiftUI

/// Settings → Records: the data Cedar keeps for the user across conversations (a running log,
/// weekly measurements), in named sets, as the web UI's Settings → Records lists them. A tap
/// opens a set's rows; swipe to delete a set, or Delete All after asking. Deletions apply at once,
/// like the language, not on Save: the next message, in any conversation, goes without them.
struct RecordsView: View {
    @Environment(AppModel.self) private var app
    @State private var confirmingDeleteAll = false
    @State private var actionError: String?

    var body: some View {
        List {
            if let error = app.recordsError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .font(.footnote)
                }
            }
            if !app.recordsLoaded {
                if app.recordsError == nil {
                    ProgressView().frame(maxWidth: .infinity)
                }
            } else if !app.recordsSupported {
                ContentUnavailableView("Records", systemImage: "tablecells",
                                       description: Text("This server doesn't keep records between conversations yet. Update cedar to use records."))
            } else if app.recordSets.isEmpty {
                ContentUnavailableView("No Records Yet", systemImage: "tablecells",
                                       description: Text("When you give Cedar data to come back to, like a weekly export or your measurements, it saves the rows here and can compare them week by week."))
            } else {
                Section {
                    ForEach(sortedSets) { set in
                        NavigationLink {
                            RecordSetView(set: set)
                        } label: {
                            row(set)
                        }
                        .swipeActions {
                            Button(role: .destructive) {
                                delete(set)
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                    }
                } header: {
                    Text(countText)
                } footer: {
                    Text("Cedar is told each set's name, size and columns, never the rows, until it looks them up. Records stay on your own server and are never shared with a third party. Settings → Tools in the web UI turns them off.")
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Theme.canvas)
        .navigationTitle("Records")
        .toolbar {
            if !app.recordSets.isEmpty {
                ToolbarItem(placement: .primaryAction) {
                    Button("Delete All", role: .destructive) { confirmingDeleteAll = true }
                }
            }
        }
        .task { await app.refreshRecords() }
        .refreshable { await app.refreshRecords() }
        .alert("Delete Every Set?", isPresented: $confirmingDeleteAll) {
            Button("Delete All", role: .destructive) { deleteAll() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text(deleteAllMessage)
        }
        .alert("Could Not Do That", isPresented: Binding(
            get: { actionError != nil },
            set: { if !$0 { actionError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(actionError ?? "")
        }
    }

    /// Most recently saved first.
    private var sortedSets: [RecordSet] {
        app.recordSets.sorted { $0.updatedAt > $1.updatedAt }
    }

    /// One set: its name, description, size, key, columns and when it was last saved.
    private func row(_ set: RecordSet) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(set.name).font(.body.monospaced()).fontWeight(.semibold)
                if set.healthData {
                    Text("Health data")
                        .font(.caption2)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 1)
                        .background(Theme.raised, in: Capsule())
                }
            }
            if !set.description.isEmpty {
                Text(set.description).font(.subheadline)
            }
            Text(verbatim: "\(rowsText(set)) · \(set.updatedAt.formatted(.relative(presentation: .named).locale(L10n.locale)))")
                .font(.footnote)
                .foregroundStyle(.secondary)
            if !set.columns.isEmpty {
                Text(set.columns.joined(separator: ", "))
                    .font(.footnote.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
        }
    }

    private func rowsText(_ set: RecordSet) -> String {
        L10n.format(set.count == 1 ? "%lld row" : "%lld rows", set.count)
    }

    private var countText: String {
        let count = app.recordSets.count
        if let max = app.recordsLimits?.maxSets, max > 0 {
            return L10n.format("%lld of %lld sets", count, max)
        }
        return L10n.format(count == 1 ? "%lld set" : "%lld sets", count)
    }

    private var deleteAllMessage: String {
        let count = app.recordSets.count
        return L10n.format(count == 1
                           ? "Cedar will delete the %lld set of records it keeps for you. This can't be undone."
                           : "Cedar will delete all %lld sets of records it keeps for you. This can't be undone.", count)
    }

    private func delete(_ set: RecordSet) {
        Task {
            do {
                try await app.deleteRecordSet(set)
            } catch {
                actionError = error.localizedDescription
                await app.refreshRecords()
            }
        }
    }

    private func deleteAll() {
        Task {
            do {
                try await app.deleteAllRecords()
            } catch {
                actionError = error.localizedDescription
                await app.refreshRecords()
            }
        }
    }
}

/// One set's rows, newest first: each row's fields, when it was saved, and the conversation it
/// came from (a tap opens it).
struct RecordSetView: View {
    @Environment(AppModel.self) private var app
    let set: RecordSet
    @State private var detail: RecordSetDetail?
    @State private var error: String?

    var body: some View {
        List {
            if let error {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .font(.footnote)
                }
            } else if let detail {
                if detail.records.isEmpty {
                    ContentUnavailableView("No Rows", systemImage: "tablecells",
                                           description: Text("This set holds no rows yet."))
                } else {
                    Section {
                        ForEach(detail.records.reversed()) { record in
                            row(record, columns: columns(detail))
                        }
                    } header: {
                        Text(header(detail))
                    } footer: {
                        if !set.description.isEmpty { Text(set.description) }
                    }
                }
            } else {
                ProgressView().frame(maxWidth: .infinity)
            }
        }
        .scrollContentBackground(.hidden)
        .background(Theme.canvas)
        .navigationTitle(set.name)
        .task { await load() }
        .refreshable { await load() }
    }

    private func load() async {
        do {
            detail = try await app.loadRecordSet(set.name)
            error = nil
        } catch is CancellationError {
        } catch let error as URLError where error.code == .cancelled {
        } catch {
            self.error = error.localizedDescription
        }
    }

    private func columns(_ detail: RecordSetDetail) -> [String] {
        if !detail.set.columns.isEmpty { return detail.set.columns }
        var seen: [String] = []
        for record in detail.records {
            for key in record.data.keys where !seen.contains(key) { seen.append(key) }
        }
        return seen
    }

    private func header(_ detail: RecordSetDetail) -> String {
        let count = detail.records.count
        let rows = L10n.format(count == 1 ? "%lld row" : "%lld rows", count)
        guard !detail.set.keyFields.isEmpty else { return rows }
        return "\(rows) · " + L10n.format("key: %@", detail.set.keyFields.joined(separator: ", "))
    }

    @ViewBuilder
    private func row(_ record: RecordRow, columns: [String]) -> some View {
        let content = VStack(alignment: .leading, spacing: 4) {
            ForEach(columns, id: \.self) { column in
                if let value = record.data[column], value != .null {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(column)
                            .font(.footnote.monospaced())
                            .foregroundStyle(.secondary)
                            .frame(minWidth: 72, alignment: .leading)
                        Text(value.displayText)
                            .font(.subheadline)
                            .lineLimit(3)
                    }
                }
            }
            Text(verbatim: "\(record.updatedAt.formatted(.relative(presentation: .named).locale(L10n.locale))) · \(source(record))")
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        if let conversationId = record.conversationId {
            Button {
                app.showingSettings = false
                app.selection = conversationId
                app.compactColumn = .detail
            } label: {
                content
            }
            .foregroundStyle(.primary)
        } else {
            content
        }
    }

    private func source(_ record: RecordRow) -> String {
        if record.conversationId != nil {
            let title = record.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            return title.isEmpty ? L10n.string("New chat") : title
        }
        return L10n.string(record.runId == nil ? "Uploaded by hand" : "From a conversation not in this app")
    }
}

private extension JSONValue {
    /// The value as a row shows it: strings as they are, numbers without a trailing `.0`, the
    /// rest as JSON.
    var displayText: String {
        switch self {
        case .string(let s): return s
        case .number(let n): return n == n.rounded() && abs(n) < 1e15 ? String(Int64(n)) : String(n)
        case .bool(let b): return b ? "true" : "false"
        case .null: return ""
        default:
            let data = (try? JSONEncoder().encode(self)) ?? Data()
            return String(decoding: data, as: UTF8.self)
        }
    }
}
