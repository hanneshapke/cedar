import Foundation

/// One Server-Sent Events message.
struct SSEMessage: Equatable, Sendable {
    var event = "message"
    var data = ""
    var id: String?
}

/// An incremental Server-Sent Events parser. Feed it lines (without their terminator); it returns
/// a message when a blank line ends one.
///
/// `URLSession.AsyncBytes.lines` drops blank lines, which carry meaning in SSE, so callers split
/// the byte stream themselves (see `SSELineSplitter`).
struct SSEParser {
    private var event: String?
    private var data: [String] = []
    private var id: String?

    mutating func feed(line: String) -> SSEMessage? {
        if line.isEmpty {
            defer { event = nil; data = []; id = nil }
            guard !data.isEmpty else { return nil }
            return SSEMessage(event: event ?? "message", data: data.joined(separator: "\n"), id: id)
        }
        if line.hasPrefix(":") { return nil }  // comment, e.g. keep-alive

        let field: Substring
        var value: Substring
        if let colon = line.firstIndex(of: ":") {
            field = line[..<colon]
            value = line[line.index(after: colon)...]
            if value.hasPrefix(" ") { value = value.dropFirst() }
        } else {
            field = Substring(line)
            value = ""
        }
        switch field {
        case "event": event = String(value)
        case "data": data.append(String(value))
        case "id": id = String(value)
        default: break  // includes "retry"
        }
        return nil
    }
}

/// Splits a byte stream into lines on `\n`, `\r\n`, or `\r`, keeping empty lines.
struct SSELineSplitter {
    private var buffer: [UInt8] = []
    private var lastWasCR = false

    /// Returns the line completed by `byte`, if any.
    mutating func feed(_ byte: UInt8) -> String? {
        switch byte {
        case UInt8(ascii: "\n"):
            if lastWasCR {
                lastWasCR = false
                return nil
            }
            return flush()
        case UInt8(ascii: "\r"):
            lastWasCR = true
            return flush()
        default:
            lastWasCR = false
            buffer.append(byte)
            return nil
        }
    }

    private mutating func flush() -> String {
        defer { buffer.removeAll(keepingCapacity: true) }
        return String(decoding: buffer, as: UTF8.self)
    }
}
