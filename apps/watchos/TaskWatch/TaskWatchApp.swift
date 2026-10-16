// Task watch app — entry point. One screen (HomeView): Capture opens
// dictation in one tap, Timer starts the clock in one tap, and a running
// timer takes the screen. Settings is behind the gear.

import SwiftUI

@main
struct TaskWatchApp: App {
    @State private var store = TaskStore()
    // Inherits the paired iPhone's active server + session over WCSession.
    @State private var phoneSync = PhoneSync()

    var body: some Scene {
        WindowGroup {
            // One screen: capture, the timer, what is next (HomeView).
            HomeView()
            .environment(store)
            .onAppear { phoneSync.start(store: store) }
        }
    }
}
