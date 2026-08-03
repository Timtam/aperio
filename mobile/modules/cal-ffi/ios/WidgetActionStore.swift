import Foundation

/// The app's end of the widget's action queue.
///
/// A widget button cannot complete a task itself: completion cascades to parents
/// and children, self-assigns on shared lists, advances a recurring series,
/// appends to the event log and queues a sync push. None of that is reachable
/// from an extension process. So the widget writes what the user ASKED for, one
/// file per tap, and the app performs it through the same path every other
/// surface uses.
///
/// One file per action, never a shared one: two processes write here, and a
/// read-modify-write on a common file loses whichever tap lost the race.
enum WidgetActionStore {
  /// Must match `targets/widget/Actions.swift`.
  static let appGroup = "group.com.aperio.mobile"
  static let directoryName = "actions"

  private static var directory: URL? {
    FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
      .appendingPathComponent(directoryName, isDirectory: true)
  }

  /// Every queued action, as a JSON array. Each entry carries the file's `id`
  /// so the caller can clear exactly the ones it has dealt with.
  ///
  /// Returns `[]` for a missing container or an unreadable directory — on a
  /// build without the App Group, or before any widget has ever been tapped,
  /// there is simply nothing queued.
  static func pendingJson() -> String {
    guard
      let directory,
      let files = try? FileManager.default.contentsOfDirectory(
        at: directory, includingPropertiesForKeys: nil
      )
    else {
      return "[]"
    }
    var entries: [[String: Any]] = []
    for file in files where file.pathExtension == "json" {
      guard
        let data = try? Data(contentsOf: file),
        var object = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
      else {
        // A half-written or corrupt file is dropped rather than retried
        // forever. Nothing is lost that the user cannot redo in the app.
        try? FileManager.default.removeItem(at: file)
        continue
      }
      object["id"] = file.deletingPathExtension().lastPathComponent
      entries.append(object)
    }
    guard
      let json = try? JSONSerialization.data(withJSONObject: entries),
      let text = String(data: json, encoding: .utf8)
    else {
      return "[]"
    }
    return text
  }

  /// Drop one queued action by its id.
  ///
  /// Called after the app has ATTEMPTED it, not only after success. A tap that
  /// cannot be carried out — a task deleted meanwhile, a list gone — would
  /// otherwise sit in the queue permanently, and because the widget hides
  /// anything queued, the row would stay invisible forever.
  static func clear(_ id: String) {
    guard let directory else { return }
    // Guard against a caller-supplied path rather than a bare name.
    guard !id.contains("/") && !id.contains("..") else { return }
    try? FileManager.default.removeItem(
      at: directory.appendingPathComponent("\(id).json")
    )
  }
}
