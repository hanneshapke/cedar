import XCTest
@testable import Cedar

final class TailscaleTests: XCTestCase {
    private func settings(_ url: String) -> ServerSettings {
        ServerSettings(serverURL: url, accessToken: "", model: "", reasoning: "")
    }

    func testRecognizesTailnetHosts() {
        XCTAssertTrue(ServerSettings.isTailnetHost("m.tail1234.ts.net"))
        XCTAssertTrue(ServerSettings.isTailnetHost("M.Tail1234.TS.NET."))
        XCTAssertTrue(ServerSettings.isTailnetHost("100.64.0.1"))
        XCTAssertTrue(ServerSettings.isTailnetHost("100.127.255.254"))
        XCTAssertTrue(ServerSettings.isTailnetHost("[fd7a:115c:a1e0::1]"))
        XCTAssertFalse(ServerSettings.isTailnetHost("100.128.0.1"))
        XCTAssertFalse(ServerSettings.isTailnetHost("100.63.0.1"))
        XCTAssertFalse(ServerSettings.isTailnetHost("192.168.1.10"))
        XCTAssertFalse(ServerSettings.isTailnetHost("chat.example.com"))
        XCTAssertFalse(ServerSettings.isTailnetHost("ts.net.example.com"))
    }

    func testSaysWhenTheServerIsReachedThroughTailscale() {
        XCTAssertTrue(settings("https://m.t.ts.net").needsTailscale)
        XCTAssertTrue(settings("https://100.101.102.103").needsTailscale)
        XCTAssertTrue(settings("https://[fd7a:115c:a1e0::1]").needsTailscale)
        // Nothing usable yet, or the device's own address: the usual setup is tailscale serve.
        XCTAssertTrue(settings("").needsTailscale)
        XCTAssertTrue(settings(ServerSettings.defaultServerURL).needsTailscale)
        XCTAssertTrue(settings("m.t.ts.net").needsTailscale)
        // A public HTTPS server (Caddy, Let's Encrypt) needs no Tailscale.
        XCTAssertFalse(settings("https://chat.example.com").needsTailscale)
        XCTAssertFalse(settings("https://192.168.1.10").needsTailscale)
    }
}
