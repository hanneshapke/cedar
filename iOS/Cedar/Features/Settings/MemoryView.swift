import SwiftUI

/// Settings → Memory: what Cedar remembers about the user across conversations, as the web UI's
/// Settings → Memory shows it. At the top, the profile Cedar reads before every reply (written by
/// Cedar, or by the user: edit it and it stays as written). Below, a picker for the memories
/// about the user, past conversations, ways of working, or the archive, newest first, each with
/// the words it was taken from and the conversation it came from. Swipe a row to pin, archive,
/// restore or delete it; Delete All forgets everything after asking. Changes apply at once, like
/// the language, not on Save: the next message, in any conversation, sees them.
struct MemoryView: View {
    @Environment(AppModel.self) private var app
    @State private var confirmingDeleteAll = false
    @State private var actionError: String?
    @State private var profileDraft = ""
    @State private var savingProfile = false

    var body: some View {
        @Bindable var app = app
        List {
            if let error = app.memoryError {
                Section {
                    Label(error, systemImage: "exclamationmark.triangle")
                        .foregroundStyle(.secondary)
                        .font(.footnote)
                }
            }
            if !app.memoryLoaded {
                if app.memoryError == nil {
                    ProgressView().frame(maxWidth: .infinity)
                }
            } else if !app.memorySupported {
                ContentUnavailableView("Memory", systemImage: "bookmark",
                                       description: Text("This server doesn't remember anything between conversations yet. Update cedar to use memory."))
            } else {
                profileSection
                Section {
                    Picker("Show", selection: $app.memoryFilter) {
                        Text("About You").tag(MemoryFilter.about)
                        Text("Conversations").tag(MemoryFilter.conversations)
                        Text("Ways of Working").tag(MemoryFilter.procedures)
                        Text("Archive").tag(MemoryFilter.archive)
                    }
                    .pickerStyle(.menu)
                }
                if app.memoryItems.isEmpty {
                    Section {
                        Text(emptyText)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                } else {
                    Section {
                        // Newest first here; the agent reads them oldest first.
                        ForEach(app.memoryItems.reversed()) { item in
                            row(item)
                                .swipeActions(edge: .leading) {
                                    if item.isActive && item.isAboutUser {
                                        Button {
                                            change(item, pinned: !item.pinned)
                                        } label: {
                                            Label(item.pinned ? "Unpin" : "Pin", systemImage: item.pinned ? "pin.slash" : "pin")
                                        }
                                        .tint(.orange)
                                    }
                                }
                                .swipeActions(edge: .trailing) {
                                    Button(role: .destructive) {
                                        delete(item)
                                    } label: {
                                        Label("Delete", systemImage: "trash")
                                    }
                                    if item.isActive {
                                        Button {
                                            change(item, archived: true)
                                        } label: {
                                            Label("Archive", systemImage: "archivebox")
                                        }
                                    } else if item.supersededBy == nil {
                                        Button {
                                            change(item, archived: false)
                                        } label: {
                                            Label("Restore", systemImage: "arrow.uturn.backward")
                                        }
                                        .tint(.blue)
                                    }
                                }
                        }
                    } header: {
                        Text(countText)
                    } footer: {
                        Text("Pinned memories are in every reply; archived ones leave Cedar's prompt and stay here until deleted. Memory stays on your own server and is never shared with a third party. Settings → Tools in the web UI turns it off.")
                    }
                }
            }
        }
        .scrollContentBackground(.hidden)
        .background(Theme.canvas)
        .navigationTitle("Memory")
        .toolbar {
            if !app.memoryItems.isEmpty || app.memoryProfile != nil {
                ToolbarItem(placement: .primaryAction) {
                    Button("Delete All", role: .destructive) { confirmingDeleteAll = true }
                }
            }
        }
        .task { await app.refreshMemory() }
        .refreshable { await app.refreshMemory() }
        .onChange(of: app.memoryFilter) {
            Task { await app.refreshMemory() }
        }
        .onChange(of: app.memoryProfile?.text, initial: true) {
            profileDraft = app.memoryProfile?.text ?? ""
        }
        .alert("Forget Everything?", isPresented: $confirmingDeleteAll) {
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

    /// The profile: a text field with Save, and who wrote the current one when.
    private var profileSection: some View {
        Section {
            TextField("Who you are and how you like things done, in a few sentences.", text: $profileDraft, axis: .vertical)
                .lineLimit(3...8)
            if profileDraft != (app.memoryProfile?.text ?? "") {
                Button(savingProfile ? "Saving…" : "Save Profile") { saveProfile() }
                    .disabled(savingProfile)
            }
        } header: {
            Text("Profile")
        } footer: {
            Text(profileFooter)
        }
    }

    private var profileFooter: String {
        guard let profile = app.memoryProfile else {
            return L10n.string("No profile yet. Cedar writes one after a few conversations, or you can write it now.")
        }
        let when = profile.updatedAt.formatted(.relative(presentation: .named).locale(L10n.locale))
        return profile.userEdited
            ? L10n.format("Written by you %@. Cedar won't change it; clear it to let Cedar write it again.", when)
            : L10n.format("Written by Cedar %@. Edit it and it stays as you wrote it.", when)
    }

    /// One memory: its kind and text, the words it was taken from, when it was saved and the
    /// conversation it came from (a tap opens it).
    @ViewBuilder
    private func row(_ item: MemoryItem) -> some View {
        let content = VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(kindLabel(item.kind))
                    .font(.caption2)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Color.secondary.opacity(0.15), in: Capsule())
                if item.pinned {
                    Image(systemName: "pin.fill").font(.caption2).foregroundStyle(.secondary)
                }
            }
            Text(item.text)
            if let support = item.support, support != item.text {
                Text(L10n.format("You said: “%@”", support))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
            }
            Text(verbatim: detail(item))
                .font(.footnote)
                .foregroundStyle(.secondary)
                .lineLimit(1)
        }
        if let conversationId = item.conversationId {
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

    private func kindLabel(_ kind: String) -> String {
        switch kind {
        case "preference": return L10n.string("preference")
        case "instruction": return L10n.string("instruction")
        case "temporary": return L10n.string("until a date")
        case "episode": return L10n.string("conversation")
        case "procedure": return L10n.string("way of working")
        default: return L10n.string("fact")
        }
    }

    private func detail(_ item: MemoryItem) -> String {
        var bits = [(item.validFrom ?? item.createdAt).formatted(.relative(presentation: .named).locale(L10n.locale))]
        if let until = item.validUntil {
            bits.append(L10n.format("until %@", until.formatted(date: .abbreviated, time: .omitted)))
        }
        if item.supersededBy != nil {
            bits.append(L10n.string("replaced by a newer memory"))
        } else if item.archivedAt != nil {
            bits.append(L10n.string("archived"))
        }
        if item.recallCount > 0 {
            bits.append(L10n.format(item.recallCount == 1 ? "used %lld time" : "used %lld times", item.recallCount))
        }
        bits.append(source(item))
        return bits.joined(separator: " · ")
    }

    private func source(_ item: MemoryItem) -> String {
        guard item.conversationId != nil else {
            return L10n.string(item.source == "consolidation" ? "Written by Cedar from earlier memories" : "Not linked to a conversation")
        }
        let title = item.title?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return title.isEmpty ? L10n.string("New chat") : title
    }

    private var emptyText: String {
        switch app.memoryFilter {
        case .about:
            return L10n.string("When you tell Cedar something lasting, like your name, a preference or a project, or ask it to remember something, it appears here.")
        case .conversations:
            return L10n.string("No conversations noted yet. Once a conversation ends, what it was about is kept here, so Cedar can pick it up when you refer to it.")
        case .procedures:
            return L10n.string("Nothing learned yet. When a task with tools goes well, Cedar notes how it did it, in general terms, for the next time you ask.")
        case .archive:
            return L10n.string("The archive is empty. Facts that changed, temporary ones past their date, and what you or Cedar forgot stay here until you remove them.")
        }
    }

    private var countText: String {
        let count = app.memoryItems.count
        return L10n.format(count == 1 ? "%lld memory" : "%lld memories", count)
    }

    private var deleteAllMessage: String {
        L10n.string("Cedar will forget everything it remembers about you, the profile and the archive included. This can't be undone.")
    }

    private func change(_ item: MemoryItem, pinned: Bool? = nil, archived: Bool? = nil) {
        Task {
            do {
                try await app.updateMemory(item, pinned: pinned, archived: archived)
            } catch {
                actionError = error.localizedDescription
                await app.refreshMemory()
            }
        }
    }

    private func saveProfile() {
        savingProfile = true
        Task {
            defer { savingProfile = false }
            do {
                try await app.saveMemoryProfile(profileDraft)
            } catch {
                actionError = error.localizedDescription
            }
        }
    }

    private func delete(_ item: MemoryItem) {
        Task {
            do {
                try await app.deleteMemory(item)
            } catch {
                actionError = error.localizedDescription
                await app.refreshMemory()
            }
        }
    }

    private func deleteAll() {
        Task {
            do {
                try await app.deleteAllMemory()
            } catch {
                actionError = error.localizedDescription
                await app.refreshMemory()
            }
        }
    }
}
