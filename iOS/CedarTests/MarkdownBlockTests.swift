import XCTest
@testable import Cedar

final class MarkdownBlockTests: XCTestCase {
    func testParsesCommonBlocks() {
        let source = """
        # Title

        Some **bold** text
        on two lines.

        ```python
        print("hi")

        x = 1
        ```

        - one
        - two
          continued
        3. three

        > quoted

        | a | b |
        |---|:-:|
        | 1 | 2 |
        | 3 |

        ---
        """
        XCTAssertEqual(MarkdownBlock.parse(source), [
            .heading(level: 1, text: "Title"),
            .paragraph("Some **bold** text\non two lines."),
            .code(language: "python", code: "print(\"hi\")\n\nx = 1"),
            .list(ordered: false, start: 0, items: ["one", "two\ncontinued"]),
            .list(ordered: true, start: 3, items: ["three"]),
            .quote("quoted"),
            .table(header: ["a", "b"], rows: [["1", "2"], ["3", ""]]),
            .rule,
        ])
    }

    func testUnclosedFenceRunsToEnd() {
        XCTAssertEqual(MarkdownBlock.parse("```\nlet x = 1"), [.code(language: "", code: "let x = 1")])
    }

    func testBoldIsNotAListOrRule() {
        XCTAssertEqual(MarkdownBlock.parse("**note** this"), [.paragraph("**note** this")])
    }

    func testCitationsLinkToTheirSources() throws {
        let options = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
        let text = try AttributedString(markdown: "Up 4% [1][12], see `a[1]` and [docs](https://docs.example) [3].", options: options)
        let one = try XCTUnwrap(URL(string: "https://one.example"))
        let twelve = try XCTUnwrap(URL(string: "https://twelve.example"))
        let linked = Citations.link(text, sources: [1: one, 12: twelve])
        let links = linked.runs.compactMap { run in run.link.map { "\(String(linked[run.range].characters)) \($0.absoluteString)" } }
        // The code span, the existing link and the number without a source are left alone.
        XCTAssertEqual(links, ["[1] https://one.example", "[12] https://twelve.example", "docs https://docs.example"])
        XCTAssertEqual(String(linked.characters), String(text.characters))
        XCTAssertEqual(Citations.link(text, sources: [:]), text)
    }
}
