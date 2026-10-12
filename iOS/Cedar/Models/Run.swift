import Foundation

enum RunStatus: String, Codable, Sendable {
    case queued, running, completed, failed, cancelled

    var isTerminal: Bool { self != .queued && self != .running }
}

struct Usage: Codable, Hashable, Sendable {
    var inputTokens: Int
    var outputTokens: Int

    static let zero = Usage(inputTokens: 0, outputTokens: 0)

    enum CodingKeys: String, CodingKey {
        case inputTokens = "input_tokens"
        case outputTokens = "output_tokens"
    }
}

/// A cedar run, as returned by `/v1/runs`. One run answers one prompt; a follow-up is a new run
/// that continues it (`parentId`).
struct Run: Codable, Identifiable, Hashable, Sendable {
    let id: String
    var status: RunStatus
    var prompt: String
    var model: String
    var turns: Int
    var finalText: String?
    var stopReason: String?
    var error: String?
    var usage: Usage
    var parentId: String?
    var createdAt: Date
    var finishedAt: Date?

    enum CodingKeys: String, CodingKey {
        case id, status, prompt, model, turns, error, usage
        case finalText = "final_text"
        case stopReason = "stop_reason"
        case parentId = "parent_id"
        case createdAt = "created_at"
        case finishedAt = "finished_at"
    }
}

struct RunList: Decodable, Sendable {
    let runs: [Run]
    let nextBefore: String?

    enum CodingKeys: String, CodingKey {
        case runs
        case nextBefore = "next_before"
    }
}

/// One entry of a run's event log, as delivered over Server-Sent Events.
struct RunEvent: Decodable, Hashable, Sendable {
    let seq: Int
    let type: String
    let data: JSONValue

    static let terminalTypes: Set<String> = ["run.completed", "run.failed", "run.cancelled"]

    var isTerminal: Bool { Self.terminalTypes.contains(type) }
}

extension JSONDecoder {
    /// Decodes cedar's payloads, including RFC 3339 timestamps with up to nanosecond precision.
    static let cedar: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom { decoder in
            let container = try decoder.singleValueContainer()
            let raw = try container.decode(String.self)
            guard let date = CedarDate.parse(raw) else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "bad date \(raw)")
            }
            return date
        }
        return decoder
    }()
}

enum CedarDate {
    /// Parses RFC 3339 timestamps. Foundation's ISO 8601 parser only takes millisecond precision,
    /// and chrono writes up to nine fractional digits, so the fraction is trimmed first.
    static func parse(_ raw: String) -> Date? {
        var text = raw
        if let dot = text.firstIndex(of: ".") {
            let fractionStart = text.index(after: dot)
            let fractionEnd = text[fractionStart...].firstIndex { !$0.isNumber } ?? text.endIndex
            var fraction = String(text[fractionStart..<fractionEnd].prefix(3))
            while fraction.count < 3 { fraction += "0" }
            text.replaceSubrange(fractionStart..<fractionEnd, with: fraction)
            return withFraction.date(from: text)
        }
        return withoutFraction.date(from: text)
    }

    private static let withFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private static let withoutFraction: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()
}
