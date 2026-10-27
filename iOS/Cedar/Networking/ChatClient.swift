import Foundation

enum ChatError: LocalizedError, Equatable {
    case notConfigured
    case invalidURL
    case unreachable(String)
    case unauthorized
    case api(status: Int, code: String, message: String)
    case badResponse(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: return L10n.string("Add your cedar-chat server in Settings.")
        case .invalidURL: return L10n.string("The server URL is not valid.")
        case .unreachable(let detail): return L10n.format("Could not reach cedar-chat: %@", detail)
        case .unauthorized: return L10n.string("cedar-chat wants an access token, or rejected this one. Check Settings.")
        // The server's own messages are for developers and stay in English.
        case .api(_, _, let message): return message
        case .badResponse(let detail): return L10n.format("Unexpected response from cedar-chat: %@", detail)
        }
    }

    var status: Int? {
        if case .api(let status, _, _) = self { return status }
        return nil
    }
}

/// A client for cedar-chat's HTTP API (`/api`). The browser's page talks to the same routes;
/// the cedar key itself stays on the server.
struct ChatClient: Sendable {
    let baseURL: URL
    let accessToken: String
    let session: URLSession

    static let requestTimeout: TimeInterval = 30

    init(baseURL: URL, accessToken: String, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.accessToken = accessToken
        self.session = session
    }

    /// Builds a client from the user's settings.
    init(settings: ServerSettings, session: URLSession = .shared) throws {
        guard !settings.serverURL.trimmingCharacters(in: .whitespaces).isEmpty else { throw ChatError.notConfigured }
        guard let url = URL(string: settings.serverURL.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https", url.host != nil
        else { throw ChatError.invalidURL }
        self.init(baseURL: url, accessToken: settings.accessToken.trimmingCharacters(in: .whitespacesAndNewlines), session: session)
    }

    /// Where the web UI shows a conversation, for the "open on the web" links.
    func webURL(conversationId: String) -> URL {
        var text = baseURL.absoluteString
        while text.hasSuffix("/") { text.removeLast() }
        return URL(string: "\(text)/#/c/\(conversationId)") ?? baseURL
    }

    // MARK: Server

    func config() async throws -> ServerConfig {
        try await decode(makeRequest("api/config"))
    }

    func models() async throws -> [ModelInfo] {
        let list: ModelList = try await decode(makeRequest("api/models"))
        return list.models
    }

    /// Checks that the server is up and takes the token (or needs none).
    func verify() async throws {
        let config = try await self.config()
        if config.authRequired && accessToken.isEmpty { throw ChatError.unauthorized }
        _ = try await conversations()
    }

    // MARK: Conversations

    func conversations() async throws -> [Conversation] {
        let list: ConversationList = try await decode(makeRequest("api/conversations"))
        return list.conversations
    }

    func conversation(id: String) async throws -> ConversationDetail {
        try await decode(makeRequest("api/conversations/\(id)"))
    }

    func createConversation(title: String? = nil) async throws -> Conversation {
        var body: [String: String] = [:]
        if let title { body["title"] = title }
        return try await decode(makeRequest("api/conversations", method: "POST", json: body))
    }

    func rename(conversationId: String, title: String) async throws -> Conversation {
        try await decode(makeRequest("api/conversations/\(conversationId)", method: "PATCH", json: ["title": title]))
    }

    /// Deletes the conversation and its cedar runs. 409 while a response is running.
    func delete(conversationId: String) async throws {
        _ = try await send(makeRequest("api/conversations/\(conversationId)", method: "DELETE"))
    }

    /// The conversation has been looked at: its finished responses leave the inbox.
    @discardableResult
    func markSeen(conversationId: String) async throws -> Conversation {
        try await decode(makeRequest("api/conversations/\(conversationId)/seen", method: "POST"))
    }

    /// Starred conversations, most recently starred first: chats and scheduled runs alike.
    func starred() async throws -> [Conversation] {
        let list: ConversationList = try await decode(makeRequest("api/starred"))
        return list.conversations
    }

    /// Stars or unstars the conversation, for every device. Both are idempotent.
    func setStarred(conversationId: String, starred: Bool) async throws -> Conversation {
        try await decode(makeRequest("api/conversations/\(conversationId)/star", method: starred ? "PUT" : "DELETE"))
    }

    // MARK: Schedules

    func schedules() async throws -> [Schedule] {
        let list: ScheduleList = try await decode(makeRequest("api/schedules"))
        return list.schedules
    }

    func schedule(id: String) async throws -> ScheduleDetail {
        try await decode(makeRequest("api/schedules/\(id)"))
    }

    /// Starts a run now, without moving the next scheduled one. 409 while the latest is going.
    func runSchedule(id: String) async throws -> ScheduleRun {
        try await decode(makeRequest("api/schedules/\(id)/run", method: "POST"))
    }

    /// Pauses or resumes. Resuming skips the slots missed while paused.
    func setSchedule(id: String, enabled: Bool) async throws -> Schedule {
        try await decode(makeRequest("api/schedules/\(id)", method: "PATCH", json: ["enabled": enabled]))
    }

    // MARK: Messages and turns

    struct SendMessage: Encodable {
        var content: String
        var model: String?
        var reasoning: String?
        /// PDFs for any model; images for one registered as accepting them.
        var attachments: [Upload]?
        /// `off`, `on`, `auto` or `tool`; absent leaves it to the server's Settings.
        var plan: String?
        /// The interface language (`de-CH`), which the reply is written in.
        var locale: String?

        struct Upload: Encodable {
            var name: String
            var mediaType: String
            /// The file's bytes, base64.
            var data: String

            enum CodingKeys: String, CodingKey {
                case name, data
                case mediaType = "media_type"
            }
        }
    }

    /// Starts a response. 409 if one is still running in the conversation.
    func send(conversationId: String, _ message: SendMessage) async throws -> Turn {
        try await decode(makeRequest("api/conversations/\(conversationId)/messages", method: "POST", json: message))
    }

    func cancel(turnId: String) async throws {
        _ = try await send(makeRequest("api/turns/\(turnId)/cancel", method: "POST"))
    }

    /// Agrees to a run's plan, as proposed or edited.
    func answerPlan(turnId: String, steps: [PlanStep]) async throws {
        let body: [String: [[String: String]]] = ["steps": steps.map { ["id": $0.id, "title": $0.title] }]
        _ = try await send(makeRequest("api/turns/\(turnId)/plan", method: "POST", json: body))
    }

    /// `allow`, `allow_always` or `deny`.
    func decideApproval(turnId: String, approvalId: String, decision: String) async throws {
        _ = try await send(makeRequest("api/turns/\(turnId)/approvals/\(approvalId)", method: "POST", json: ["decision": decision]))
    }

    /// Streams a turn's events after `after`: a replay of the stored log, then live events. The
    /// stream finishes after the terminal event, or when the server closes the connection.
    func turnEvents(turnId: String, after: Int = 0) -> AsyncThrowingStream<RunEvent, Error> {
        let request = makeRequest("api/turns/\(turnId)/events", query: [URLQueryItem(name: "after", value: String(after))])
        return stream(request) { $0.isTerminal }
    }

    // MARK: Inbox and notices

    func inbox() async throws -> Inbox {
        try await decode(makeRequest("api/inbox"))
    }

    /// The server's live notices: a `turn.finished` for every response that ends, with no replay.
    /// Read the inbox when connecting. Stays open until cancelled or the connection drops.
    func notices() -> AsyncThrowingStream<Notice, Error> {
        stream(makeRequest("api/events")) { _ in false }
    }

    // MARK: Files

    func files(conversationId: String) async throws -> WorkspaceFiles {
        try await decode(makeRequest("api/conversations/\(conversationId)/files"))
    }

    /// An authorized request for a workspace file, for image loading and sharing.
    func fileRequest(conversationId: String, path: String) -> URLRequest {
        // `makeRequest` appends each segment as a path component, which percent-encodes it.
        makeRequest("api/conversations/\(conversationId)/files/\(path)")
    }

    // MARK: Settings

    /// `PUT /api/settings/locale`: a tag, or `null` to follow the server's default.
    struct LocaleSetting: Encodable {
        var locale: String?

        enum CodingKeys: String, CodingKey {
            case locale
        }

        func encode(to encoder: Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            // `null` is sent rather than left out: it clears the setting.
            if let locale {
                try container.encode(locale, forKey: .locale)
            } else {
                try container.encodeNil(forKey: .locale)
            }
        }
    }

    /// Tells the server the interface language, for what it writes on its own: scheduled runs and
    /// notifications.
    func setLocale(_ tag: String?) async throws {
        _ = try await send(makeRequest("api/settings/locale", method: "PUT", json: LocaleSetting(locale: tag)))
    }

    // MARK: Push

    /// Tells the server this phone's APNs device token, so it can send iPhone pushes here.
    func registerDevice(token: String, name: String?) async throws {
        var body: [String: String] = ["token": token]
        if let name, !name.isEmpty { body["name"] = name }
        _ = try await send(makeRequest("api/notifications/apns/devices", method: "POST", json: body))
    }

    func unregisterDevice(token: String) async throws {
        _ = try await send(makeRequest("api/notifications/apns/devices", method: "DELETE", json: ["token": token]))
    }

    // MARK: Plumbing

    private func makeRequest(_ path: String, method: String = "GET", query: [URLQueryItem] = []) -> URLRequest {
        var url = baseURL
        for part in path.split(separator: "/") {
            url.appendPathComponent(String(part))
        }
        if !query.isEmpty, var components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            components.queryItems = query
            url = components.url ?? url
        }
        var request = URLRequest(url: url, timeoutInterval: Self.requestTimeout)
        request.httpMethod = method
        if !accessToken.isEmpty {
            request.setValue("Bearer \(accessToken)", forHTTPHeaderField: "Authorization")
        }
        return request
    }

    private func makeRequest<T: Encodable>(_ path: String, method: String, json body: T) -> URLRequest {
        var request = makeRequest(path, method: method)
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try? JSONEncoder().encode(body)
        return request
    }

    private func send(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await Self.transport(baseURL) { try await session.data(for: request) }
        guard let http = response as? HTTPURLResponse else { throw ChatError.badResponse("not HTTP") }
        guard (200..<300).contains(http.statusCode) else { throw Self.error(status: http.statusCode, body: data) }
        return data
    }

    private func decode<T: Decodable>(_ request: URLRequest) async throws -> T {
        let data = try await send(request)
        do {
            return try JSONDecoder.cedar.decode(T.self, from: data)
        } catch {
            throw ChatError.badResponse(String(describing: error))
        }
    }

    /// A Server-Sent Events stream of JSON messages decoded as `T`, finishing after a message
    /// `isLast` says ends it, or when the connection closes.
    private func stream<T: Decodable & Sendable>(_ request: URLRequest, isLast: @escaping @Sendable (T) -> Bool) -> AsyncThrowingStream<T, Error> {
        var streamRequest = request
        streamRequest.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        // The stream stays open for as long as the run goes; the server sends a keep-alive every
        // 15 seconds.
        streamRequest.timeoutInterval = 60
        let finalRequest = streamRequest
        let session = session
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (bytes, response) = try await Self.transport(baseURL) { try await session.bytes(for: finalRequest) }
                    if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                        var body = Data()
                        for try await byte in bytes { body.append(byte) }
                        throw Self.error(status: http.statusCode, body: body)
                    }
                    var splitter = SSELineSplitter()
                    var parser = SSEParser()
                    for try await byte in bytes {
                        guard let line = splitter.feed(byte), let message = parser.feed(line: line) else { continue }
                        guard let item = try? JSONDecoder.cedar.decode(T.self, from: Data(message.data.utf8)) else {
                            continue
                        }
                        continuation.yield(item)
                        if isLast(item) { break }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    /// Maps transport failures (but not cancellation) to `ChatError.unreachable`.
    private static func transport<T>(_ baseURL: URL, _ body: () async throws -> T) async throws -> T {
        do {
            return try await body()
        } catch let error as URLError where error.code != .cancelled {
            throw unreachable(error, baseURL: baseURL)
        }
    }

    /// A TLS failure on an https URL with an explicit port usually means the port serves plain
    /// HTTP (cedar-chat itself on :3000), not the `tailscale serve` address, so say so.
    static func unreachable(_ error: URLError, baseURL: URL) -> ChatError {
        let tlsCodes: Set<URLError.Code> = [
            .secureConnectionFailed, .serverCertificateUntrusted, .serverCertificateHasBadDate,
            .serverCertificateNotYetValid, .serverCertificateHasUnknownRoot,
        ]
        guard tlsCodes.contains(error.code), baseURL.scheme == "https",
              let port = baseURL.port, port != 443 else {
            return .unreachable(error.localizedDescription)
        }
        return .unreachable(L10n.format(
            "%@ Port %lld may serve plain HTTP. Use the HTTPS address from tailscale serve, e.g. https://<machine>.<tailnet>.ts.net (no port).",
            error.localizedDescription, port))
    }

    static func error(status: Int, body: Data) -> ChatError {
        if status == 401 { return .unauthorized }
        struct Envelope: Decodable {
            struct Inner: Decodable {
                let code: String
                let message: String
            }
            let error: Inner
        }
        if let envelope = try? JSONDecoder().decode(Envelope.self, from: body) {
            return .api(status: status, code: envelope.error.code, message: envelope.error.message)
        }
        return .api(status: status, code: "http_\(status)", message: L10n.format("cedar-chat returned HTTP %lld", status))
    }
}
