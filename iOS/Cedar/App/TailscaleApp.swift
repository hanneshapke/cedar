import UIKit

/// The Tailscale app, which the device needs to reach a server served with `tailscale serve`.
enum TailscaleApp {
    /// The scheme the Tailscale app registers; `canOpenURL` may ask about it because it is listed
    /// under `LSApplicationQueriesSchemes` in Config/Info.plist.
    private static let probe = URL(string: "tailscale://")!

    static let appStoreURL = URL(string: "https://apps.apple.com/app/tailscale/id1470499037")!

    @MainActor static var isInstalled: Bool {
        UIApplication.shared.canOpenURL(probe)
    }
}
