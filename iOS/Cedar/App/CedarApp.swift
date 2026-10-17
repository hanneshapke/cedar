import SwiftUI

@main
struct CedarApp: App {
    @UIApplicationDelegateAdaptor(PushRegistrar.self) private var push
    @State private var app = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(app)
                .task {
                    push.attach(app)
                    app.start()
                }
        }
    }
}
