import AppIntents
import Foundation
import WidgetKit

// Ticking a task off FROM the widget.
//
// The extension cannot do the work itself. Completing a task in Aperio is not a
// flag flip: it cascades to parents and children, self-assigns on shared lists,
// advances a recurring series, appends to the event log and queues a sync push.
// That machinery is the Rust core plus the app's own rules, and none of it is
// reachable from a process with a few tens of megabytes and no bridge.
//
// So the widget does not write the answer; it writes the QUESTION. One file per
// tap into the App Group, which the app drains through its ordinary completion
// path the next time it runs — on a foreground, or on the background sync round
// that happens without the user opening anything.
//
// One file per action, never one shared file: two processes append here, and a
// read-modify-write on a common file loses whichever tap lost the race.

let actionsDirectoryName = "actions"

/// A queued tap. `version` lets an older app skip a shape it does not know
/// rather than misread it — the app and the extension update together, but the
/// queue outlives both across an update.
struct PendingAction: Codable {
    let version: Int
    /// Only `complete` for now. A string, so an older app reading a newer
    /// queue skips the entry instead of failing to decode the file.
    let action: String
    /// The task id exactly as the snapshot carried it.
    let itemId: String
    /// Its list — the app needs it to resolve the task's behaviour settings.
    let containerId: String
    /// When the tap happened, so a drain long afterwards can still be honest
    /// about it.
    let at: String
}

let currentActionVersion = 1

enum ActionQueue {
    private static var directory: URL? {
        FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
            .appendingPathComponent(actionsDirectoryName, isDirectory: true)
    }

    /// Queue a tap. Silent on failure: a widget button that reports an error has
    /// nowhere to report it, and the task is still there to tick in the app.
    static func enqueue(_ action: PendingAction) {
        guard let directory else { return }
        try? FileManager.default.createDirectory(
            at: directory, withIntermediateDirectories: true
        )
        guard let data = try? JSONEncoder().encode(action) else { return }
        // A fresh name per tap — see the note above about two writers.
        let file = directory.appendingPathComponent("\(UUID().uuidString).json")
        try? data.write(to: file, options: .atomic)
    }

    /// Item ids with a queued completion.
    ///
    /// The widget renders against this as well as the snapshot: the snapshot
    /// still lists a task the app has not processed yet, and a row that stays
    /// put after being tapped reads as a button that does nothing.
    static func pendingCompletions() -> Set<String> {
        guard
            let directory,
            let files = try? FileManager.default.contentsOfDirectory(
                at: directory, includingPropertiesForKeys: nil
            )
        else {
            return []
        }
        var ids: Set<String> = []
        for file in files {
            guard
                let data = try? Data(contentsOf: file),
                let action = try? JSONDecoder().decode(PendingAction.self, from: data),
                action.action == "complete"
            else {
                continue
            }
            ids.insert(action.itemId)
        }
        return ids
    }
}

/// The button in a widget row.
///
/// Runs in the background when tapped — no app launch, no screen change. That is
/// the whole point on a lock or home screen, and it is also why `perform` must
/// stay this small.
struct CompleteTaskIntent: AppIntent {
    static var title: LocalizedStringResource = "Complete task"
    /// Explicitly false: opening the app would defeat the purpose, and would
    /// pull a screen-reader user out of the home screen they were reading.
    static var openAppWhenRun: Bool = false

    @Parameter(title: "Item") var itemId: String
    @Parameter(title: "List") var containerId: String

    init() {}

    init(itemId: String, containerId: String) {
        self.itemId = itemId
        self.containerId = containerId
    }

    func perform() async throws -> some IntentResult {
        ActionQueue.enqueue(
            PendingAction(
                version: currentActionVersion,
                action: "complete",
                itemId: itemId,
                containerId: containerId,
                at: ISO8601DateFormatter().string(from: Date())
            )
        )
        // Redraw now, so the row disappears under the finger rather than after
        // the app next happens to run.
        WidgetCenter.shared.reloadAllTimelines()
        return .result()
    }
}
