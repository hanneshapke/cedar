import SwiftUI

struct ChatView: View {
    @Environment(AppModel.self) private var app
    @Bindable var chat: ChatModel
    @FocusState private var composerFocused: Bool

    private let bottomID = "bottom"

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 20) {
                    if chat.exchanges.isEmpty && chat.pendingPrompt == nil {
                        if chat.isLoading {
                            ProgressView().frame(maxWidth: .infinity).padding(.top, 40)
                        } else if chat.conversationId == nil {
                            Greeting()
                        }
                    }
                    ForEach(chat.exchanges) { exchange in
                        ExchangeView(exchange: exchange)
                    }
                    if let pending = chat.pendingPrompt {
                        UserBubble(text: pending)
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
        .task { await chat.start() }
        .onAppear {
            if chat.conversationId == nil { composerFocused = true }
        }
    }

    private var title: String {
        if let id = chat.conversationId, let conversation = app.conversation(id: id) {
            return app.title(for: conversation)
        }
        return chat.exchanges.first.map { Conversation(runs: [$0.run]).defaultTitle } ?? "New Chat"
    }

    /// Changes whenever new content arrives, to keep the newest content in view.
    private var scrollSignature: [Int] {
        [chat.exchanges.count, chat.exchanges.last?.lastSeq ?? 0, chat.pendingPrompt == nil ? 0 : 1]
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
            Text("It can write files and run commands in its own sandbox. Follow-ups keep its files.")
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

    var body: some View {
        HStack {
            Spacer(minLength: 48)
            Text(text)
                .textSelection(.enabled)
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

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            TextField("Message cedar", text: $chat.draft, axis: .vertical)
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
        .padding(.horizontal)
        .padding(.vertical, 8)
        .frame(maxWidth: 820)
        .frame(maxWidth: .infinity)
        .background(.bar)
    }
}
