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
    /// For the requests the server answers only after a model or a tool has run.
    static let slowRequestTimeout: TimeInterval = 300

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

    // MARK: Memory

    /// What cedar remembers about the user, as `filter` says. A server without the route reads
    /// as not supported.
    func memory(_ filter: MemoryFilter = .about) async throws -> MemoryList {
        var query: [URLQueryItem] = []
        if let kind = filter.kind {
            query.append(URLQueryItem(name: "kind", value: kind))
        }
        if filter.archived {
            query.append(URLQueryItem(name: "archived", value: "true"))
        }
        do {
            return try await decode(makeRequest("api/memory", query: query))
        } catch let error as ChatError where error.status == 404 {
            return .unsupported
        }
    }

    /// How many active memories there are, of every kind; `nil` on a cedar from before memory.
    func memoryCount() async throws -> Int? {
        let query = [URLQueryItem(name: "kind", value: MemoryFilter.allKinds)]
        do {
            let list: MemoryList = try await decode(makeRequest("api/memory", query: query))
            return list.supported ? list.items.count : nil
        } catch let error as ChatError where error.status == 404 {
            return nil
        }
    }

    /// Pins, unpins, archives or restores one memory; cedar answers with the item.
    func updateMemory(id: String, _ change: MemoryChange) async throws -> MemoryItem {
        try await decode(makeRequest("api/memory/\(id)", method: "PATCH", json: change))
    }

    /// Writes the profile by hand (an empty text clears it); it then stays as written.
    func setMemoryProfile(_ text: String) async throws -> MemoryProfile? {
        let response: MemoryProfileResponse = try await decode(makeRequest("api/memory/profile", method: "PUT", json: ["text": text]))
        return response.profile
    }

    /// Forgets one memory; it applies from the next message on, in every conversation.
    func deleteMemory(id: String) async throws {
        _ = try await send(makeRequest("api/memory/\(id)", method: "DELETE"))
    }

    /// Forgets everything cedar remembers about the user.
    func deleteAllMemory() async throws {
        _ = try await send(makeRequest("api/memory", method: "DELETE"))
    }

    // MARK: Records

    /// The sets cedar keeps for the user. A server without the route reads as not supported.
    func records() async throws -> RecordsList {
        do {
            return try await decode(makeRequest("api/records"))
        } catch let error as ChatError where error.status == 404 {
            return .unsupported
        }
    }

    /// One set with all its rows.
    func recordSet(name: String) async throws -> RecordSetDetail {
        try await decode(makeRequest("api/records/\(name)"))
    }

    /// Deletes one set; it applies from the next message on, in every conversation.
    func deleteRecordSet(name: String) async throws {
        _ = try await send(makeRequest("api/records/\(name)", method: "DELETE"))
    }

    /// Deletes every set cedar keeps for the user.
    func deleteAllRecords() async throws {
        _ = try await send(makeRequest("api/records", method: "DELETE"))
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

    /// Starred conversations, most recently starred first: chats and agents' runs alike.
    func starred() async throws -> [Conversation] {
        let list: ConversationList = try await decode(makeRequest("api/starred"))
        return list.conversations
    }

    /// The conversations with every word of `query` in their title or messages, most recently
    /// active first: chats and agents' runs alike, each with the passage that matched.
    func search(query: String) async throws -> [Conversation] {
        let list: ConversationList = try await decode(makeRequest("api/conversations/search", query: [URLQueryItem(name: "q", value: query)]))
        return list.conversations
    }

    /// Stars or unstars the conversation, for every device. Both are idempotent.
    func setStarred(conversationId: String, starred: Bool) async throws -> Conversation {
        try await decode(makeRequest("api/conversations/\(conversationId)/star", method: starred ? "PUT" : "DELETE"))
    }

    // MARK: Contributing a conversation

    /// The conversation as it would be contributed; `reasoning` and `fetched` include what is left out by default.
    func contributionPreview(conversationId: String, reasoning: Bool = false, fetched: Bool = false) async throws -> ContributionPreview {
        var query: [URLQueryItem] = []
        if reasoning { query.append(URLQueryItem(name: "reasoning", value: "true")) }
        if fetched { query.append(URLQueryItem(name: "fetched", value: "true")) }
        return try await decode(makeRequest("api/conversations/\(conversationId)/contribution", query: query))
    }

    /// Send the bundle as shown (the outcome inside it); answers the receipt.
    func sendContribution(conversationId: String, bundle: JSONValue) async throws -> Contribution {
        try await decode(makeRequest("api/conversations/\(conversationId)/contribution", method: "POST",
                                     json: SendContribution(bundle: bundle, outcome: bundle["outcome"])))
    }

    /// The person's thumb on a response: 1, -1 or none.
    func rateTurn(turnId: String, rating: Int?) async throws -> Turn {
        try await decode(makeRequest("api/turns/\(turnId)/rating", method: "PUT", json: Rating(rating: rating)))
    }

    func contributions() async throws -> [Contribution] {
        let list: ContributionList = try await decode(makeRequest("api/contributions"))
        return list.contributions
    }

    /// Ask the corpus what became of each contribution; answers the list with statuses.
    func refreshContributions() async throws -> [Contribution] {
        let list: ContributionList = try await decode(makeRequest("api/contributions/refresh", method: "POST"))
        return list.contributions
    }

    func withdrawContribution(id: String) async throws -> Contribution {
        try await decode(makeRequest("api/contributions/\(id)", method: "DELETE"))
    }

    func contributionSettings() async throws -> ContributionSettings {
        try await decode(makeRequest("api/settings/contributions"))
    }

    func updateContributionSettings(_ patch: ContributionSettingsPatch) async throws -> ContributionSettings {
        try await decode(makeRequest("api/settings/contributions", method: "PUT", json: patch))
    }

    private struct Rating: Encodable {
        /// Encoded as `null` when absent, which clears the rating.
        let rating: Int?

        func encode(to encoder: Encoder) throws {
            var c = encoder.container(keyedBy: CodingKeys.self)
            try c.encode(rating, forKey: .rating)
        }

        enum CodingKeys: String, CodingKey { case rating }
    }

    // MARK: Agents

    /// Every agent, enabled first, with the server's limits and defaults.
    func agents() async throws -> AgentList {
        try await decode(makeRequest("api/agents"))
    }

    /// One agent and its runs, newest first.
    func agent(id: String) async throws -> AgentDetail {
        try await decode(makeRequest("api/agents/\(id)"))
    }

    /// 409 once there are as many agents as the server keeps. A webhook agent's answer carries
    /// its secret, this once.
    func createAgent(_ body: AgentBody) async throws -> Agent {
        try await decode(makeRequest("api/agents", method: "POST", json: body))
    }

    /// Changes what `body` sets. `enabled` pauses or resumes; resuming skips what was missed
    /// while paused.
    func updateAgent(id: String, _ body: AgentBody) async throws -> Agent {
        try await decode(makeRequest("api/agents/\(id)", method: "PATCH", json: body))
    }

    /// 409 while one of its runs is still going.
    func deleteAgent(id: String) async throws {
        _ = try await send(makeRequest("api/agents/\(id)", method: "DELETE"))
    }

    /// Starts a run now, without moving the next one. 409 while the latest is going.
    func runAgent(id: String) async throws -> AgentRun {
        try await decode(makeRequest("api/agents/\(id)/run", method: "POST"))
    }

    /// A new secret for a webhook agent; the old one stops working at once. The answer is the
    /// only time the new one is shown.
    func rotateHook(id: String) async throws -> Agent {
        try await decode(makeRequest("api/agents/\(id)/hook/rotate", method: "POST"))
    }

    /// One check of a watching trigger, with nothing kept. A check calls a tool or a web
    /// address, so it gets longer than other requests.
    func testTrigger(_ trigger: Trigger) async throws -> TriggerTest {
        var request = makeRequest("api/agents/test", method: "POST", json: TriggerTestBody(trigger: trigger))
        request.timeoutInterval = Self.slowRequestTimeout
        return try await decode(request)
    }

    /// The latest runs of every agent, newest first. `before` is the last page's `nextBefore`.
    func activity(before: String? = nil, limit: Int? = nil) async throws -> ActivityPage {
        var query: [URLQueryItem] = []
        if let before { query.append(URLQueryItem(name: "before", value: before)) }
        if let limit { query.append(URLQueryItem(name: "limit", value: String(limit))) }
        return try await decode(makeRequest("api/agents/activity", query: query))
    }

    /// cedar's tools by group, for an agent's tools and a condition's tool source.
    func tools() async throws -> ToolCatalog {
        try await decode(makeRequest("api/tools"))
    }

    /// The default notification method and the ones that are set up.
    func notificationStatus() async throws -> NotificationStatus {
        try await decode(makeRequest("api/notifications"))
    }

    /// Has the model turn a finished conversation into one prompt, for making an agent from
    /// it. 404 when it has no finished response yet. The model writes it, so it takes a while.
    func compact(conversationId: String) async throws -> Compacted {
        var request = makeRequest("api/conversations/\(conversationId)/compact", method: "POST")
        request.timeoutInterval = Self.slowRequestTimeout
        return try await decode(request)
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

    /// Tells the server the interface language, for what it writes on its own: agents' runs and
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
            // `URLComponents` leaves "+" as it is, and the server reads one as a space.
            components.percentEncodedQuery = components.percentEncodedQuery?.replacingOccurrences(of: "+", with: "%2B")
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
        // The port goes in as text: a number would get the locale's grouping ("3,000").
        return .unreachable(L10n.format(
            "%@ Port %@ may serve plain HTTP. Use the HTTPS address from tailscale serve, e.g. https://<machine>.<tailnet>.ts.net (no port).",
            error.localizedDescription, String(port)))
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
