import Foundation
import UIKit
import UserNotifications

/// Registers this phone for iPhone pushes from cedar-chat (Settings → Notifications → iPhone
/// push on the server): asks for permission, hands APNs's device token to the server, and
/// opens the conversation a tapped notification names.
@MainActor
final class PushRegistrar: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    static private(set) var shared: PushRegistrar?

    private weak var app: AppModel?
    private var deviceToken: String? {
        get { UserDefaults.standard.string(forKey: "apnsDeviceToken") }
        set { UserDefaults.standard.set(newValue, forKey: "apnsDeviceToken") }
    }
    /// The server the token was last sent to, so a change of server moves the registration.
    private var registeredServer: String? {
        get { UserDefaults.standard.string(forKey: "apnsRegisteredServer") }
        set { UserDefaults.standard.set(newValue, forKey: "apnsRegisteredServer") }
    }
    /// A conversation named by a notification tapped before the app model was attached.
    private var pendingOpen: String?

    override init() {
        super.init()
        PushRegistrar.shared = self
    }

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        UNUserNotificationCenter.current().delegate = self
        return true
    }

    /// Called once the app model exists. Asks for permission when the server is configured and a
    /// language has been picked (a new install asks for that first; `AppModel.chooseLanguage` asks
    /// for permission then).
    func attach(_ app: AppModel) {
        self.app = app
        if let pendingOpen {
            self.pendingOpen = nil
            app.open(conversationId: pendingOpen)
        }
        if app.settings.isConfigured, LanguageStore.shared.hasChosen {
            Task { await requestAuthorization() }
        }
    }

    /// Asks the user, then APNs; the token comes back through the delegate.
    func requestAuthorization() async {
        let center = UNUserNotificationCenter.current()
        let settings = await center.notificationSettings()
        switch settings.authorizationStatus {
        case .notDetermined:
            guard (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) == true else { return }
        case .denied:
            return
        default:
            break
        }
        UIApplication.shared.registerForRemoteNotifications()
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let token = deviceToken.map { String(format: "%02x", $0) }.joined()
        self.deviceToken = token
        Task { await register() }
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        // Without a token the server can't push here; the inbox and the list still work.
        print("push registration failed: \(error.localizedDescription)")
    }

    /// Sends the token to the current server, if it isn't there already.
    private func register() async {
        guard let app, let client = app.client, let token = deviceToken else { return }
        do {
            try await client.registerDevice(token: token, name: UIDevice.current.name)
            registeredServer = client.baseURL.absoluteString
        } catch {
            print("could not register for push: \(error.localizedDescription)")
        }
    }

    /// The server (or its token) changed: take the registration along.
    func serverChanged(from old: ServerSettings, to new: ServerSettings) {
        Task {
            if let token = deviceToken, let oldClient = try? ChatClient(settings: old),
               registeredServer == oldClient.baseURL.absoluteString {
                try? await oldClient.unregisterDevice(token: token)
                registeredServer = nil
            }
            if new.isConfigured {
                await requestAuthorization()
                await register()
            }
        }
    }

    // MARK: UNUserNotificationCenterDelegate

    /// A push that arrives while the app is in front: shown unless its conversation is open.
    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        let conversationId = notification.request.content.userInfo["conversation_id"] as? String
        if let app, let conversationId, app.selection == conversationId {
            app.markSeen(conversationId: conversationId)
            return []
        }
        return [.banner, .list, .sound]
    }

    /// A tapped notification opens its conversation.
    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let conversationId = response.notification.request.content.userInfo["conversation_id"] as? String else { return }
        if let app {
            app.open(conversationId: conversationId)
        } else {
            pendingOpen = conversationId
        }
    }
}
