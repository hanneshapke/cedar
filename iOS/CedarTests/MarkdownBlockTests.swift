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
}
