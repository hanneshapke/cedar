import PhotosUI
import SwiftUI

struct ChatView: View {
    @Environment(AppModel.self) private var app
    @Bindable var chat: ChatModel
    @FocusState private var composerFocused: Bool
    @State private var starError: String?
    @State private var showingFiles = false
    @State private var showingContribute = false
    /// The detail of the agent whose run this is, and the form that makes an agent from this chat.
    @State private var showingAgent: AgentSelection?
    @State private var makingAgent: AgentFormContext?
    /// The agent the form just made: its detail opens once the form is gone.
    @State private var madeAgent: AgentSelection?

    private let bottomID = "bottom"

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 20) {
                    if chat.turns.isEmpty && chat.pendingPrompt == nil {
                        if chat.isLoading {
                            ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                        } else if chat.conversationId == nil {
                            Greeting()
                        }
                    }
                    ForEach(chat.turns) { turn in
                        TurnView(turn: turn, chat: chat, webURL: webURL, isLast: turn.id == chat.turns.last?.id,
                                 openAgent: { agentId, title in
                                     showingAgent = AgentSelection(id: agentId, title: title)
                                 })
                    }
                    if let questions = chat.openQuestions, chat.pendingPrompt == nil {
                        QuestionsView(questions: questions) { answers in
                            Task { await chat.answer(questions, with: answers) }
                        }
                        // Its own identity per turn (a fresh form for new questions), never the turn's
                        // id itself: the ForEach above already uses that, and a lazy stack draws only
                        // one of two views that share an id.
                        .id("questions-\(chat.turns.last?.id ?? "")")
                    }
                    if let pending = chat.pendingPrompt {
                        UserBubble(text: pending, attachments: chat.pendingAttachments)
                        ProgressView().padding(.leading, 4)
                    }
                    if let error = chat.errorMessage {
                        ErrorBanner(message: error) { chat.errorMessage = nil }
                    }
                    Color.clear.frame(height: 1).id(bottomID)
                }
                .padding(.horizontal)
                .padding(.top, 12)
                .frame(maxWidth: 820)
                .frame(maxWidth: .infinity)
            }
            .background(Theme.canvas)
            .scrollDismissesKeyboard(.interactively)
            .defaultScrollAnchor(.bottom)
            .onChange(of: scrollSignature) {
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(bottomID, anchor: .bottom) }
            }
        }
        .safeAreaInset(edge: .bottom) {
            Composer(chat: chat, focused: $composerFocused)
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            // An agent's run says whose it is; the badge opens the agent.
            if let agent = runAgent {
                AgentBadge(agent: agent) {
                    showingAgent = AgentSelection(id: agent.id, title: agent.title)
                }
            }
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbarBackground(Theme.canvas, for: .navigationBar)
        .toolbar {
            if let id = chat.conversationId {
                let starred = app.isStarred(conversationId: id)
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        Task {
                            do {
                                try await app.setStarred(conversationId: id, !starred)
                            } catch {
                                starError = error.localizedDescription
                            }
                        }
                    } label: {
                        Label(starred ? "Unstar" : "Star", systemImage: starred ? "star.fill" : "star")
                    }
                    .tint(starred ? Color.yellow : nil)
                }
            }
            if chat.conversationId != nil, app.serverHasContributions {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showingContribute = true
                    } label: {
                        Label("Contribute", systemImage: "square.and.arrow.up")
                    }
                }
            }
            if canMakeAgent {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        makeAgent()
                    } label: {
                        Label("Make Agent", systemImage: "clock.arrow.circlepath")
                    }
                }
            }
            if chat.conversationId != nil, chat.client != nil {
                ToolbarItem(placement: .topBarTrailing) {
                    Button {
                        showingFiles = true
                    } label: {
                        Label("Files", systemImage: "folder")
                    }
                }
            }
            if let webURL {
                ToolbarItem(placement: .topBarTrailing) {
                    Link(destination: webURL) {
                        Label("Open on the web", systemImage: "safari")
                    }
                }
            }
        }
        .alert("Could Not Change the Star", isPresented: Binding(
            get: { starError != nil },
            set: { if !$0 { starError = nil } }
        )) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(starError ?? "")
        }
        .sheet(isPresented: $showingContribute) {
            if let id = chat.conversationId {
                ContributeView(conversationId: id)
            }
        }
        .sheet(isPresented: $showingFiles) {
            if let id = chat.conversationId, let client = chat.client {
                WorkspaceFilesView(conversationId: id, client: client)
            }
        }
        .sheet(item: $showingAgent) { selection in
            AgentDetailView(agentId: selection.id, title: selection.title)
        }
        .sheet(item: $makingAgent, onDismiss: {
            // The new agent's detail follows its form: a webhook's secret is shown only there.
            if let madeAgent {
                self.madeAgent = nil
                showingAgent = madeAgent
            }
        }) { context in
            AgentFormView(context: context) { agent in
                madeAgent = AgentSelection(id: agent.id, title: agent.title)
            }
        }
        .task { await chat.start() }
        .onAppear {
            if chat.conversationId == nil { composerFocused = true }
        }
    }

    private var title: String {
        if let id = chat.conversationId, let conversation = app.conversation(id: id) {
            return app.title(for: conversation)
        }
        return chat.turns.first.map { firstLine($0.prompt) } ?? L10n.string("New Chat")
    }

    /// The agent whose run this conversation is, if it is one.
    private var runAgent: AgentRef? {
        chat.conversationId.flatMap { app.agent(ofConversation: $0) }
    }

    /// A chat with a finished response can become an agent; an agent's own run can't.
    private var canMakeAgent: Bool {
        chat.conversationId != nil && runAgent == nil && !chat.isActive
            && chat.turns.contains { $0.turn.status == .completed }
    }

    /// Opens the agent form for this conversation; the server writes the prompt from it.
    private func makeAgent() {
        guard let id = chat.conversationId else { return }
        let conversation = app.conversation(id: id)
        makingAgent = AgentFormContext(conversationId: id,
                                       title: conversation.map { app.title(for: $0) } ?? "",
                                       model: conversation?.model ?? "",
                                       reasoning: conversation?.reasoning ?? "",
                                       limits: app.agentLimits)
    }

    private var webURL: URL? {
        guard let id = chat.conversationId, let client = app.client else { return nil }
        return client.webURL(conversationId: id)
    }

    private func firstLine(_ text: String) -> String {
        let line = text.split(whereSeparator: \.isNewline).first.map { $0.trimmingCharacters(in: .whitespaces) } ?? L10n.string("New Chat")
        return line.count > 80 ? String(line.prefix(80)) + "…" : line
    }

    /// Changes whenever new content arrives, to keep the newest content in view.
    private var scrollSignature: [Int] {
        [chat.turns.count, chat.turns.last?.lastSeq ?? 0, chat.pendingPrompt == nil ? 0 : 1, chat.openQuestions == nil ? 0 : 1]
    }
}

private struct Greeting: View {
    var body: some View {
        VStack(spacing: 16) {
            CedarBadge()
                .padding(.bottom, 4)
            Text("What should the agent do?")
                .font(.title2.weight(.semibold))
                .multilineTextAlignment(.center)
            Text("It can search the web, write files and run code in its own sandbox. Follow-ups keep its files.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .frame(maxWidth: 340)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 72)
        .padding(.horizontal)
    }
}

struct UserBubble: View {
    let text: String
    var attachments: [AttachmentInfo] = []

    var body: some View {
        HStack {
            Spacer(minLength: 48)
            VStack(alignment: .trailing, spacing: 6) {
                Text(text)
                    .textSelection(.enabled)
                if !attachments.isEmpty {
                    HStack(spacing: 6) {
                        ForEach(attachments, id: \.self) { file in
                            Label(file.name, systemImage: file.mediaType == "application/pdf" ? "doc.richtext" : "photo")
                                .font(.caption)
                                .lineLimit(1)
                                .padding(.horizontal, 8)
                                .padding(.vertical, 4)
                                .background(Color.white.opacity(0.18), in: Capsule())
                        }
                    }
                }
            }
            .foregroundStyle(Theme.onGreen)
            .tint(Theme.onGreen)
            .padding(.horizontal, 15)
            .padding(.vertical, 10)
            .background(Theme.green, in: UnevenRoundedRectangle(topLeadingRadius: 20, bottomLeadingRadius: 20,
                                                                bottomTrailingRadius: 6, topTrailingRadius: 20,
                                                                style: .continuous))
        }
    }
}

private struct ErrorBanner: View {
    let message: String
    let dismiss: () -> Void

    var body: some View {
        HStack(alignment: .top) {
            Image(systemName: "exclamationmark.triangle.fill")
            Text(message).frame(maxWidth: .infinity, alignment: .leading)
            Button(action: dismiss) {
                Image(systemName: "xmark")
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Dismiss")
        }
        .font(.footnote)
        .foregroundStyle(.red)
        .padding(12)
        .background(Color.red.opacity(0.1), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 14, style: .continuous).strokeBorder(Color.red.opacity(0.25)))
    }
}

private struct Composer: View {
    @Bindable var chat: ChatModel
    var focused: FocusState<Bool>.Binding
    @Environment(\.verticalSizeClass) private var verticalSizeClass
    @State private var showingPhotos = false
    @State private var showingFiles = false
    @State private var photos: [PhotosPickerItem] = []

    var body: some View {
        VStack(spacing: 8) {
            if !chat.attachments.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(chat.attachments) { file in
                            AttachmentChip(file: file) { chat.removeAttachment(file) }
                        }
                    }
                }
            }
            row
        }
        .padding(.horizontal, 12)
        .padding(.top, 4)
        .padding(.bottom, 8)
        .frame(maxWidth: 820)
        .frame(maxWidth: .infinity)
        // The conversation fades out under the composer rather than stopping at a bar.
        .background {
            LinearGradient(colors: [Theme.canvas.opacity(0), Theme.canvas, Theme.canvas],
                           startPoint: .top, endPoint: .bottom)
                .padding(.top, -16)
                .ignoresSafeArea(edges: .bottom)
        }
        .photosPicker(isPresented: $showingPhotos, selection: $photos,
                      maxSelectionCount: OutgoingAttachment.maxFiles, matching: .images)
        .onChange(of: photos) {
            guard !photos.isEmpty else { return }
            let picked = photos
            photos = []
            Task { await chat.addPhotos(picked) }
        }
        .fileImporter(isPresented: $showingFiles,
                      allowedContentTypes: chat.acceptsImages ? [.pdf, .image] : [.pdf],
                      allowsMultipleSelection: true) { result in
            switch result {
            case .success(let urls):
                Task { await chat.addFiles(at: urls) }
            case .failure(let error):
                chat.errorMessage = error.localizedDescription
            }
        }
    }

    private var row: some View {
        HStack(alignment: .bottom, spacing: 4) {
            attachMenu
            TextField(chat.openQuestions == nil ? "Message cedar" : "Or answer in your own words", text: $chat.draft, axis: .vertical)
                .lineLimit(FoldLayout.composerLineLimit(verticalSizeClass: verticalSizeClass))
                .focused(focused)
                .padding(.vertical, 9)
                .padding(.horizontal, 2)
            if chat.isActive {
                Button {
                    Task { await chat.stop() }
                } label: {
                    Image(systemName: "stop.fill")
                        .font(.system(size: 13, weight: .bold))
                        .foregroundStyle(Theme.canvas)
                        .frame(width: 36, height: 36)
                        .background(Color.primary, in: Circle())
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Stop")
                .disabled(chat.pendingPrompt != nil)
            } else {
                Button {
                    Task { await chat.send() }
                } label: {
                    Image(systemName: "arrow.up")
                        .font(.system(size: 16, weight: .bold))
                        .foregroundStyle(chat.canSend ? Theme.onGreen : Color.secondary)
                        .frame(width: 36, height: 36)
                        .background(chat.canSend ? Theme.green : Theme.raised, in: Circle())
                        .animation(.snappy(duration: 0.15), value: chat.canSend)
                }
                .buttonStyle(.plain)
                .accessibilityLabel("Send")
                .disabled(!chat.canSend)
            }
        }
        .padding(5)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: 24, style: .continuous).strokeBorder(Theme.line))
        .shadow(color: .black.opacity(0.08), radius: 16, y: 4)
    }

    /// Photos (when the model takes images) and PDFs or images from Files.
    private var attachMenu: some View {
        Menu {
            if chat.acceptsImages {
                Button {
                    showingPhotos = true
                } label: {
                    Label("Photo Library", systemImage: "photo.on.rectangle")
                }
            }
            Button {
                showingFiles = true
            } label: {
                Label(chat.acceptsImages ? "Choose Files" : "Choose PDFs", systemImage: "folder")
            }
        } label: {
            Group {
                if chat.isAddingAttachments {
                    ProgressView()
                } else {
                    Image(systemName: "plus")
                        .font(.system(size: 17, weight: .semibold))
                        .foregroundStyle(.secondary)
                }
            }
            .frame(width: 36, height: 36)
            .background(Theme.raised, in: Circle())
        }
        .accessibilityLabel(chat.acceptsImages ? "Attach images or PDFs" : "Attach PDFs")
        .disabled(chat.isActive || chat.isAddingAttachments || chat.attachments.count >= OutgoingAttachment.maxFiles)
    }
}

/// A file picked for the next message, with a thumbnail for images and a remove button.
private struct AttachmentChip: View {
    let file: OutgoingAttachment
    let remove: () -> Void
    @State private var thumbnail: UIImage?

    var body: some View {
        HStack(spacing: 6) {
            Group {
                if let thumbnail {
                    Image(uiImage: thumbnail)
                        .resizable()
                        .scaledToFill()
                } else {
                    Image(systemName: file.isImage ? "photo" : "doc.richtext")
                        .foregroundStyle(.secondary)
                }
            }
            .frame(width: 28, height: 28)
            .clipShape(RoundedRectangle(cornerRadius: 6))
            VStack(alignment: .leading, spacing: 0) {
                Text(file.name)
                    .font(.caption)
                    .lineLimit(1)
                Text(OutgoingAttachment.format(file.data.count))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: 140, alignment: .leading)
            Button(action: remove) {
                Image(systemName: "xmark.circle.fill")
                    .foregroundStyle(.secondary)
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Remove \(file.name)")
        }
        .padding(.leading, 4)
        .padding(.trailing, 8)
        .padding(.vertical, 4)
        .background(Theme.surface, in: Capsule())
        .overlay(Capsule().strokeBorder(Theme.line))
        .task(id: file.id) {
            guard file.isImage else { return }
            let data = file.data
            thumbnail = await Task.detached {
                UIImage(data: data)?.preparingThumbnail(of: CGSize(width: 84, height: 84))
            }.value
        }
    }
}
