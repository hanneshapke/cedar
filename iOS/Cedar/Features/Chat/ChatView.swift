import PhotosUI
import SwiftUI

struct ChatView: View {
    @Environment(AppModel.self) private var app
    @Bindable var chat: ChatModel
    @FocusState private var composerFocused: Bool
    @State private var starError: String?
    @State private var showingFiles = false

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
                        TurnView(turn: turn, chat: chat, webURL: webURL, isLast: turn.id == chat.turns.last?.id)
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
            .scrollDismissesKeyboard(.interactively)
            .defaultScrollAnchor(.bottom)
            .onChange(of: scrollSignature) {
                withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo(bottomID, anchor: .bottom) }
            }
        }
        .safeAreaInset(edge: .bottom) {
            Composer(chat: chat, focused: $composerFocused)
        }
        .navigationTitle(title)
        .navigationBarTitleDisplayMode(.inline)
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
        .sheet(isPresented: $showingFiles) {
            if let id = chat.conversationId, let client = chat.client {
                WorkspaceFilesView(conversationId: id, client: client)
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
        VStack(spacing: 12) {
            Image(systemName: "leaf")
                .font(.system(size: 40))
                .foregroundStyle(.tint)
            Text("What should the agent do?")
                .font(.title2.weight(.semibold))
            Text("It can search the web, write files and run code in its own sandbox. Follow-ups keep its files.")
                .font(.subheadline)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 80)
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
                                .background(Color(.tertiarySystemBackground), in: Capsule())
                        }
                    }
                }
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 18))
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
        .background(Color.red.opacity(0.1), in: RoundedRectangle(cornerRadius: 12))
    }
}

private struct Composer: View {
    @Bindable var chat: ChatModel
    var focused: FocusState<Bool>.Binding
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
        .padding(.horizontal)
        .padding(.vertical, 8)
        .frame(maxWidth: 820)
        .frame(maxWidth: .infinity)
        .background(.bar)
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
        HStack(alignment: .bottom, spacing: 8) {
            attachMenu
            TextField(chat.openQuestions == nil ? "Message cedar" : "Or answer in your own words", text: $chat.draft, axis: .vertical)
                .lineLimit(1...8)
                .focused(focused)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Color(.secondarySystemBackground), in: RoundedRectangle(cornerRadius: 20))
            if chat.isActive {
                Button {
                    Task { await chat.stop() }
                } label: {
                    Image(systemName: "stop.circle.fill")
                        .font(.system(size: 34))
                }
                .accessibilityLabel("Stop")
                .disabled(chat.pendingPrompt != nil)
            } else {
                Button {
                    Task { await chat.send() }
                } label: {
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.system(size: 34))
                }
                .accessibilityLabel("Send")
                .disabled(!chat.canSend)
            }
        }
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
                    Image(systemName: "paperclip")
                        .font(.system(size: 20))
                }
            }
            .frame(width: 34, height: 34)
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
        .background(Color(.secondarySystemBackground), in: Capsule())
        .task(id: file.id) {
            guard file.isImage else { return }
            let data = file.data
            thumbnail = await Task.detached {
                UIImage(data: data)?.preparingThumbnail(of: CGSize(width: 84, height: 84))
            }.value
        }
    }
}
