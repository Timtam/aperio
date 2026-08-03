import AppIntents
import Foundation

// Aperio's Siri shortcuts.
//
// This file is NOT part of any Expo module — it is copied into the generated
// iOS app target by `plugins/withAppShortcuts.js`, and that is a requirement
// rather than a preference: an `AppShortcutsProvider` is only discovered in the
// MAIN app target, and Apple's framework-level escape hatch (`AppIntentsPackage`)
// covers frameworks only, not the static libraries Expo modules compile to.
//
// Step 1 answered the question this file was created for: a provider added this
// way IS discovered, and the shortcut runs. Step 2 puts something useful behind
// it — creating an event from a spoken title and time.
//
// The intent does NOT create the event itself. Same reason the widget's checkbox
// does not complete a task: creating one means the event log, the sync queue and
// the adapter rules, and an intent has no way to reach them. So it writes the
// REQUEST into the App Group, exactly like the widget's queue, and the app
// performs it on the way in.
//
// `openAppWhenRun` is the difference from the widget, and it is deliberate. A
// tap that vanishes a row can afford to be applied minutes later; a spoken "make
// an appointment tomorrow at eleven" cannot. You would say it, look, and find
// nothing. So the app comes forward, drains the queue and shows the result.

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
        AppShortcut(
            intent: CreateEventIntent(),
            // No date in the phrase — Siri refuses a `Date` there. It asks.
            phrases: [
                "New event in \(.applicationName)",
                "Neuer Termin in \(.applicationName)",
            ],
            shortTitle: "New event",
            systemImageName: "calendar.badge.plus"
        )
    }
}

/// The App Group where the app and its extensions leave things for each other.
/// Must match `plugins/withAppGroup.js`, `WidgetSnapshotStore` and the widget's
/// own copy — four places now, and a mismatch is silent in all of them.
private let appGroup = "group.com.aperio.mobile"
private let actionsDirectoryName = "actions"

/// Queue one request for the app to carry out.
///
/// One file per request, never a shared one: the widget's extension and this
/// intent both write here, and a read-modify-write on a common file loses
/// whichever caller lost the race.
private func enqueue(_ payload: [String: Any]) {
    guard
        let container = FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: appGroup)
    else {
        return
    }
    let directory = container.appendingPathComponent(actionsDirectoryName, isDirectory: true)
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    guard let data = try? JSONSerialization.data(withJSONObject: payload) else { return }
    try? data.write(
        to: directory.appendingPathComponent("\(UUID().uuidString).json"),
        options: .atomic
    )
}

private let isoFormatter: ISO8601DateFormatter = {
    let f = ISO8601DateFormatter()
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    f.timeZone = TimeZone(identifier: "UTC")
    return f
}()

@available(iOS 16.0, *)
struct CreateEventIntent: AppIntent {
    static var title: LocalizedStringResource = "New event"
    static var description = IntentDescription("Creates an event in Aperio.")
    /// Creating something has to be visible. See the note at the top of the file.
    static var openAppWhenRun: Bool = true

    @Parameter(title: "Title", requestValueDialog: "What should the event be called?")
    var eventTitle: String

    /// Siri resolves the spoken time itself — "tomorrow at eleven", "next
    /// Tuesday at two" — and hands over a real Date. This is why the whole
    /// feature needs no natural-language date parser of ours, and why it is iOS
    /// only: nothing on the other platform offers the same.
    ///
    /// It cannot appear in the shortcut PHRASE, though. Siri asks for it
    /// instead, which for a screen reader is arguably the better shape: each
    /// part is confirmed as it is given.
    @Parameter(title: "Date", requestValueDialog: "When?")
    var date: Date

    func perform() async throws -> some IntentResult {
        enqueue([
            "version": 1,
            "action": "createEvent",
            "title": eventTitle,
            "startsAt": isoFormatter.string(from: date),
            "at": isoFormatter.string(from: Date()),
        ])
        return .result()
    }
}
