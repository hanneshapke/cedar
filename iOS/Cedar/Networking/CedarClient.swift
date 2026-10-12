import Foundation

enum CedarError: LocalizedError, Equatable {
    case notConfigured
    case invalidURL
    case unreachable(String)
    case unauthorized
    case api(status: Int, code: String, message: String)
    case badResponse(String)

    var errorDescription: String? {
        switch self {
        case .notConfigured: return "Add your cedar server and API key in Settings."
        case .invalidURL: return "The server URL is not valid."
        case .unreachable(let detail): return "Could not reach cedar: \(detail)"
        case .unauthorized: return "cedar rejected the API key."
        case .api(_, _, let message): return message
        case .badResponse(let detail): return "Unexpected response from cedar: \(detail)"
        }
    }

    var status: Int? {
        if case .api(let status, _, _) = self { return status }
        return nil
    }
}

/// A client for cedar's HTTP API (`/v1`).
struct CedarClient: Sendable {
    let baseURL: URL
    let apiKey: String
    let session: URLSession

    static let requestTimeout: TimeInterval = 30

    init(baseURL: URL, apiKey: String, session: URLSession = .shared) {
        self.baseURL = baseURL
        self.apiKey = apiKey
        self.session = session
    }

    /// Builds a client from the user's settings.
    init(settings: ServerSettings, session: URLSession = .shared) throws {
        let key = settings.apiKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !settings.serverURL.isEmpty, !key.isEmpty else { throw CedarError.notConfigured }
        guard let url = URL(string: settings.serverURL.trimmingCharacters(in: .whitespacesAndNewlines)),
              let scheme = url.scheme?.lowercased(), scheme == "http" || scheme == "https", url.host != nil
        else { throw CedarError.invalidURL }
        self.init(baseURL: url, apiKey: key, session: session)
    }

    // MARK: Runs

    struct CreateRun: Encodable {
        var prompt: String
        var model: String?
        var system: String?
        var parentId: String?

        enum CodingKeys: String, CodingKey {
            case prompt, model, system
            case parentId = "parent_id"
        }
    }

    func createRun(_ body: CreateRun) async throws -> Run {
        var request = makeRequest("v1/runs", method: "POST")
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(body)
        return try await decode(request)
    }

    func listRuns(limit: Int = 100, before: String? = nil) async throws -> RunList {
        var query = [URLQueryItem(name: "limit", value: String(limit))]
        if let before { query.append(URLQueryItem(name: "before", value: before)) }
        return try await decode(makeRequest("v1/runs", query: query))
    }

    /// Fetches up to `maxRuns` of the newest runs, following pagination.
    func allRuns(maxRuns: Int = 500) async throws -> [Run] {
        var runs: [Run] = []
        var before: String?
        repeat {
            let page = try await listRuns(limit: min(100, maxRuns - runs.count), before: before)
            runs += page.runs
            before = page.nextBefore
        } while before != nil && runs.count < maxRuns
        return runs
    }

    func run(id: String) async throws -> Run {
        try await decode(makeRequest("v1/runs/\(id)"))
    }

    func cancelRun(id: String) async throws {
        _ = try await send(makeRequest("v1/runs/\(id)/cancel", method: "POST"))
    }

    func deleteRun(id: String) async throws {
        _ = try await send(makeRequest("v1/runs/\(id)", method: "DELETE"))
    }

    /// Streams a run's events after `after`: a replay of the stored log, then live events. The
    /// stream finishes after the terminal event, or when cedar closes the connection.
    func events(runId: String, after: Int = 0) -> AsyncThrowingStream<RunEvent, Error> {
        var request = makeRequest("v1/runs/\(runId)/events", query: [URLQueryItem(name: "after", value: String(after))])
        request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        // The stream stays open for the whole run; cedar sends a keep-alive every 15 seconds.
        request.timeoutInterval = 60
        let streamRequest = request
        let session = session
        return AsyncThrowingStream { continuation in
            let task = Task {
                do {
                    let (bytes, response) = try await Self.transport { try await session.bytes(for: streamRequest) }
                    if let http = response as? HTTPURLResponse, !(200..<300).contains(http.statusCode) {
                        var body = Data()
                        for try await byte in bytes { body.append(byte) }
                        throw Self.error(status: http.statusCode, body: body)
                    }
                    var splitter = SSELineSplitter()
                    var parser = SSEParser()
                    for try await byte in bytes {
                        guard let line = splitter.feed(byte), let message = parser.feed(line: line) else { continue }
                        guard let event = try? JSONDecoder.cedar.decode(RunEvent.self, from: Data(message.data.utf8)) else {
                            continue
                        }
                        continuation.yield(event)
                        if event.isTerminal { break }
                    }
                    continuation.finish()
                } catch {
                    continuation.finish(throwing: error)
                }
            }
            continuation.onTermination = { _ in task.cancel() }
        }
    }

    // MARK: Health

    /// Checks that the server is up and accepts the key.
    func verify() async throws {
        _ = try await listRuns(limit: 1)
    }

    // MARK: Plumbing

    private func makeRequest(_ path: String, method: String = "GET", query: [URLQueryItem] = []) -> URLRequest {
        var url = baseURL.appendingPathComponent(path)
        if !query.isEmpty, var components = URLComponents(url: url, resolvingAgainstBaseURL: false) {
            components.queryItems = query
            url = components.url ?? url
        }
        var request = URLRequest(url: url, timeoutInterval: Self.requestTimeout)
        request.httpMethod = method
        request.setValue("Bearer \(apiKey)", forHTTPHeaderField: "Authorization")
        return request
    }

    private func send(_ request: URLRequest) async throws -> Data {
        let (data, response) = try await Self.transport { try await session.data(for: request) }
        guard let http = response as? HTTPURLResponse else { throw CedarError.badResponse("not HTTP") }
        guard (200..<300).contains(http.statusCode) else { throw Self.error(status: http.statusCode, body: data) }
        return data
    }

    private func decode<T: Decodable>(_ request: URLRequest) async throws -> T {
        let data = try await send(request)
        do {
            return try JSONDecoder.cedar.decode(T.self, from: data)
        } catch {
            throw CedarError.badResponse(String(describing: error))
        }
    }

    /// Maps transport failures (but not cancellation) to `CedarError.unreachable`.
    private static func transport<T>(_ body: () async throws -> T) async throws -> T {
        do {
            return try await body()
        } catch let error as URLError where error.code != .cancelled {
            throw CedarError.unreachable(error.localizedDescription)
        }
    }

    static func error(status: Int, body: Data) -> CedarError {
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
        return .api(status: status, code: "http_\(status)", message: "cedar returned HTTP \(status)")
    }
}
