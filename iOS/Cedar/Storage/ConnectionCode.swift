import Foundation

/// What the QR code in the Cedar Mac app carries: `cedar://connect?server=<url>&token=<token>`,
/// the chat's HTTPS address on the tailnet and its access token (`desktop/src/connect.rs` makes it,
/// and `scripts/cedar-stack.sh --pair` on Linux).
struct ConnectionCode: Equatable {
    var serverURL: String
    var accessToken: String
}

extension ConnectionCode {
    /// `nil` for anything else a camera might see, and for an address the app couldn't use
    /// anyway (it connects over HTTPS only).
    init?(_ text: String) {
        guard let components = URLComponents(string: text.trimmingCharacters(in: .whitespacesAndNewlines)),
              components.scheme?.lowercased() == "cedar",
              components.host?.lowercased() == "connect"
        else { return nil }
        let items = components.queryItems ?? []
        func value(_ name: String) -> String {
            (items.first { $0.name == name }?.value ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        }
        let server = value("server")
        guard let url = URL(string: server), url.scheme?.lowercased() == "https", let host = url.host, !host.isEmpty
        else { return nil }
        serverURL = server
        accessToken = value("token")
    }
}
