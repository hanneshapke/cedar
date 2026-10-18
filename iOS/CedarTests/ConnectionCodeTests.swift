import XCTest
@testable import Cedar

final class ConnectionCodeTests: XCTestCase {
    func testReadsTheMacAppsLink() {
        // As macos/src/connect.rs writes it: form-encoded, so ':' and '/' are escaped.
        let code = ConnectionCode("cedar://connect?server=https%3A%2F%2Fmy-mac.tail1234.ts.net&token=ab%26c%3Dd")
        XCTAssertEqual(code, ConnectionCode(serverURL: "https://my-mac.tail1234.ts.net", accessToken: "ab&c=d"))
        XCTAssertEqual(ConnectionCode("cedar://connect?server=https://m.t.ts.net:8443")?.serverURL, "https://m.t.ts.net:8443")
        XCTAssertEqual(ConnectionCode("cedar://connect?server=https://m.t.ts.net")?.accessToken, "")
    }

    func testTurnsDownOtherCodes() {
        XCTAssertNil(ConnectionCode("https://example.com"))
        XCTAssertNil(ConnectionCode("cedar://other?server=https://m.t.ts.net"))
        XCTAssertNil(ConnectionCode("cedar://connect?token=abc"))
        // The app connects over HTTPS only.
        XCTAssertNil(ConnectionCode("cedar://connect?server=http://127.0.0.1:13000&token=abc"))
    }
}
