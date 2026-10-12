import Foundation

/// The block structure of a Markdown document. SwiftUI's `Text` renders inline Markdown (bold,
/// code, links) but not blocks, so blocks are split out here and drawn one by one.
enum MarkdownBlock: Hashable, Sendable {
    case heading(level: Int, text: String)
    case paragraph(String)
    case code(language: String, code: String)
    case list(ordered: Bool, start: Int, items: [String])
    case quote(String)
    case table(header: [String], rows: [[String]])
    case rule

    static func parse(_ source: String) -> [MarkdownBlock] {
        var parser = Parser(lines: source.replacingOccurrences(of: "\r\n", with: "\n").components(separatedBy: "\n"))
        return parser.run()
    }
}

private struct Parser {
    let lines: [String]
    var index = 0
    var blocks: [MarkdownBlock] = []
    var paragraph: [String] = []

    init(lines: [String]) {
        self.lines = lines
    }

    mutating func run() -> [MarkdownBlock] {
        while index < lines.count {
            let line = lines[index]
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.isEmpty {
                flushParagraph()
                index += 1
            } else if let fence = fenceMarker(trimmed) {
                flushParagraph()
                parseFence(fence: fence, info: String(trimmed.dropFirst(fence.count)))
            } else if let heading = heading(trimmed) {
                flushParagraph()
                blocks.append(heading)
                index += 1
            } else if isRule(trimmed) {
                flushParagraph()
                blocks.append(.rule)
                index += 1
            } else if trimmed.hasPrefix(">") {
                flushParagraph()
                parseQuote()
            } else if listMarker(line) != nil {
                flushParagraph()
                parseList()
            } else if trimmed.hasPrefix("|"), index + 1 < lines.count, isTableSeparator(lines[index + 1]) {
                flushParagraph()
                parseTable()
            } else {
                paragraph.append(trimmed)
                index += 1
            }
        }
        flushParagraph()
        return blocks
    }

    mutating func flushParagraph() {
        guard !paragraph.isEmpty else { return }
        blocks.append(.paragraph(paragraph.joined(separator: "\n")))
        paragraph = []
    }

    func fenceMarker(_ trimmed: String) -> String? {
        for marker in ["```", "~~~"] where trimmed.hasPrefix(marker) {
            return String(trimmed.prefix { $0 == marker.first })
        }
        return nil
    }

    mutating func parseFence(fence: String, info: String) {
        index += 1
        var code: [String] = []
        while index < lines.count {
            let trimmed = lines[index].trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix(fence), trimmed.allSatisfy({ $0 == fence.first }) {
                index += 1
                break
            }
            code.append(lines[index])
            index += 1
        }
        let language = info.trimmingCharacters(in: .whitespaces).split(separator: " ").first.map(String.init) ?? ""
        blocks.append(.code(language: language, code: code.joined(separator: "\n")))
    }

    func heading(_ trimmed: String) -> MarkdownBlock? {
        let hashes = trimmed.prefix { $0 == "#" }.count
        guard (1...6).contains(hashes) else { return nil }
        let rest = trimmed.dropFirst(hashes)
        guard rest.isEmpty || rest.first == " " else { return nil }
        var text = rest.trimmingCharacters(in: .whitespaces)
        while text.hasSuffix("#") { text.removeLast() }
        return .heading(level: hashes, text: text.trimmingCharacters(in: .whitespaces))
    }

    func isRule(_ trimmed: String) -> Bool {
        let compact = trimmed.replacingOccurrences(of: " ", with: "")
        guard compact.count >= 3, let first = compact.first, "-*_".contains(first) else { return false }
        return compact.allSatisfy { $0 == first }
    }

    mutating func parseQuote() {
        var text: [String] = []
        while index < lines.count {
            let trimmed = lines[index].trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix(">") else { break }
            var rest = trimmed.dropFirst()
            if rest.first == " " { rest = rest.dropFirst() }
            text.append(String(rest))
            index += 1
        }
        blocks.append(.quote(text.joined(separator: "\n")))
    }

    /// The list marker at the start of `line`: whether it is ordered, its number, and the text
    /// after it.
    func listMarker(_ line: String) -> (ordered: Bool, number: Int, text: String)? {
        let trimmed = line.drop { $0 == " " }
        if let first = trimmed.first, "-*+".contains(first), trimmed.dropFirst().first == " " {
            return (false, 0, String(trimmed.dropFirst(2)))
        }
        let digits = trimmed.prefix { $0.isASCII && $0.isNumber }
        guard !digits.isEmpty, digits.count <= 9 else { return nil }
        let rest = trimmed.dropFirst(digits.count)
        guard let delimiter = rest.first, delimiter == "." || delimiter == ")", rest.dropFirst().first == " " else {
            return nil
        }
        return (true, Int(digits) ?? 1, String(rest.dropFirst(2)))
    }

    mutating func parseList() {
        guard let first = listMarker(lines[index]) else { return }
        var items = [first.text]
        index += 1
        while index < lines.count {
            let line = lines[index]
            if let marker = listMarker(line), marker.ordered == first.ordered {
                items.append(marker.text)
                index += 1
            } else if line.hasPrefix("  "), !line.trimmingCharacters(in: .whitespaces).isEmpty, !items.isEmpty {
                // An indented continuation (or nested item), kept with the item above.
                items[items.count - 1] += "\n" + line.trimmingCharacters(in: .whitespaces)
                index += 1
            } else {
                break
            }
        }
        blocks.append(.list(ordered: first.ordered, start: first.number, items: items))
    }

    func isTableSeparator(_ line: String) -> Bool {
        let parts = cells(line)
        return !parts.isEmpty && parts.allSatisfy { cell in
            !cell.isEmpty && cell.allSatisfy { $0 == "-" || $0 == ":" } && cell.contains("-")
        }
    }

    func cells(_ line: String) -> [String] {
        var trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("|") { trimmed.removeFirst() }
        if trimmed.hasSuffix("|") { trimmed.removeLast() }
        return trimmed.split(separator: "|", omittingEmptySubsequences: false)
            .map { $0.trimmingCharacters(in: .whitespaces) }
    }

    mutating func parseTable() {
        let header = cells(lines[index])
        index += 2
        var rows: [[String]] = []
        while index < lines.count, lines[index].trimmingCharacters(in: .whitespaces).hasPrefix("|") {
            var row = cells(lines[index])
            if row.count < header.count { row += Array(repeating: "", count: header.count - row.count) }
            rows.append(Array(row.prefix(header.count)))
            index += 1
        }
        blocks.append(.table(header: header, rows: rows))
    }
}
