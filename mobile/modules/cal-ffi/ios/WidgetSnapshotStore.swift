import Foundation
import WidgetKit

/// The handover point between the app and its widgets.
///
/// A widget extension is a separate process with a small memory budget and no
/// way into the app sandbox. Rather than teach it to open the database and
/// re-derive an agenda, the app writes the finished answer here — one small JSON
/// document in the App Group — and the widget only decodes it.
///
/// This costs nothing in freshness. The database changes only when the app runs
/// or its background-sync task runs, and both of those refresh this file on the
/// way out. A widget reading the database directly would see the very same
/// bytes, having linked a second copy of the engine to get at them.
enum WidgetSnapshotStore {
  /// Must match `DatabaseLocation.appGroup`, `plugins/withAppGroup.js` and the
  /// widget's `expo-target.config.js`.
  static let appGroup = "group.com.aperio.mobile"
  /// The widget side hardcodes this same name — the two targets share no source.
  static let fileName = "upcoming.json"

  enum StoreError: Error {
    case noContainer
  }

  /// Replace the snapshot and tell WidgetKit its timelines are out of date.
  ///
  /// Written to a neighbouring temporary file and moved into place, because a
  /// widget can wake and read at any moment: a plain overwrite has a window in
  /// which the file is a valid path holding half a document, and the widget
  /// would render the failure as "nothing planned".
  static func write(_ json: String) throws {
    guard
      let container = FileManager.default
        .containerURL(forSecurityApplicationGroupIdentifier: appGroup)
    else {
      throw StoreError.noContainer
    }
    let target = container.appendingPathComponent(fileName)
    let scratch = container.appendingPathComponent(fileName + ".writing")
    try Data(json.utf8).write(to: scratch, options: .atomic)
    // `replaceItemAt` is the atomic swap; it removes the source on success.
    _ = try FileManager.default.replaceItemAt(target, withItemAt: scratch)
    // Without this the widget keeps showing the previous timeline until the
    // system next decides to ask — which can be hours.
    WidgetCenter.shared.reloadAllTimelines()
  }
}
