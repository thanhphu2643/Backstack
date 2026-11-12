import AppKit
import SwiftUI

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_: Notification) { NSApp.activate() }
    func applicationShouldTerminateAfterLastWindowClosed(_: NSApplication) -> Bool { true }
}

@main
struct BackstackApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var store = Store()

    var body: some Scene {
        Window("Backstack", id: "main") {
            // Relative dates ("last week") follow the chosen language too.
            ContentView().environment(store).environment(\.locale, Lang.shared.locale)
        }
        .windowResizability(.contentMinSize)
        .commands {
            CommandGroup(replacing: .appSettings) {
                Button(t("Settings…", "Cài đặt…")) { store.showSettings = true }.keyboardShortcut(",")
            }
        }
    }
}
