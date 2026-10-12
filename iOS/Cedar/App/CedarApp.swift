import SwiftUI

@main
struct CedarApp: App {
    @State private var app = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(app)
        }
    }
}
