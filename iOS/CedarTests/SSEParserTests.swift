import XCTest
@testable import Cedar

final class SSEParserTests: XCTestCase {
    private func messages(from raw: String) -> [SSEMessage] {
        var splitter = SSELineSplitter()
        var parser = SSEParser()
        var out: [SSEMessage] = []
        for byte in Array(raw.utf8) {
            if let line = splitter.feed(byte), let message = parser.feed(line: line) {
                out.append(message)
            }
        }
        return out
    }

    func testParsesEventsSeparatedByBlankLines() {
        let raw = "id: 1\nevent: run.started\ndata: {\"seq\":1}\n\n: keep-alive\n\nid: 2\nevent: usage\ndata: {\"seq\":2}\n\n"
        XCTAssertEqual(messages(from: raw), [
            SSEMessage(event: "run.started", data: "{\"seq\":1}", id: "1"),
            SSEMessage(event: "usage", data: "{\"seq\":2}", id: "2"),
        ])
    }

    func testHandlesCRLFAndMultilineData() {
        let raw = "data: a\r\ndata: b\r\n\r\ndata:c\r\r"
        XCTAssertEqual(messages(from: raw), [
            SSEMessage(event: "message", data: "a\nb", id: nil),
            SSEMessage(event: "message", data: "c", id: nil),
        ])
    }

    func testIncompleteMessageIsNotDispatched() {
        XCTAssertEqual(messages(from: "data: partial\n"), [])
    }

    func testDecodesRunEvent() throws {
        let json = #"{"seq":3,"type":"tool.call","data":{"id":"t1","name":"bash","input":{"command":"ls"}},"created_at":"2026-10-12T03:36:00.123456789Z"}"#
        let event = try JSONDecoder.cedar.decode(RunEvent.self, from: Data(json.utf8))
        XCTAssertEqual(event.seq, 3)
        XCTAssertEqual(event.type, "tool.call")
        XCTAssertEqual(event.data["input"]?["command"]?.stringValue, "ls")
    }
}
