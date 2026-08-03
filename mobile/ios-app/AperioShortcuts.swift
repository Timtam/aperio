import AppIntents

// Aperio's Siri shortcuts.
//
// This file is NOT part of any Expo module — it is copied into the generated
// iOS app target by `plugins/withAppShortcuts.js`, and that is a requirement
// rather than a preference: an `AppShortcutsProvider` is only discovered in the
// MAIN app target, and Apple's framework-level escape hatch (`AppIntentsPackage`)
// covers frameworks only, not the static libraries Expo modules compile to.
//
// Step 1 of the voice work: one shortcut that does nothing but open the app.
// It exists to answer whether a managed Expo build discovers a provider added
// this way at all — the question every later step rests on, and one that costs
// a full build to ask. Parameters, calendars, task lists and actual creation
// come after it is answered.

@available(iOS 16.0, *)
struct OpenAperioIntent: AppIntent {
    static var title: LocalizedStringResource = "Open Aperio"
    static var description = IntentDescription("Opens Aperio.")
    /// The whole point of this one: if the app comes forward, the plumbing works.
    static var openAppWhenRun: Bool = true

    func perform() async throws -> some IntentResult {
        .result()
    }
}

@available(iOS 16.0, *)
struct AperioShortcuts: AppShortcutsProvider {
    static var appShortcuts: [AppShortcut] {
        AppShortcut(
            intent: OpenAperioIntent(),
            // Every phrase MUST contain `\(.applicationName)` — Siri refuses to
            // hand a bare verb to a third-party app, which is also why "create
            // an event" alone can never reach us.
            phrases: [
                "Open \(.applicationName)",
                "Öffne \(.applicationName)",
            ],
            shortTitle: "Open Aperio",
            systemImageName: "calendar"
        )
    }
}
