import Foundation

/// The calendars and task lists a voice request is allowed to name.
///
/// A Siri intent has to resolve "which calendar?" BEFORE `perform()` runs, and
/// it has no more access to the database than a widget does — the intent is
/// plain Swift in the app target, with no bridge to the React Native layer and
/// no business opening a second SQLite connection in a process iOS may be about
/// to suspend. So the same answer as the widget: the app writes the finished
/// list where the intent can read it.
///
/// Deliberately NOT folded into `WidgetSnapshotStore`. That one nudges
/// WidgetKit on every write, which is right for an agenda that changed and
/// pointless for a list of calendar names that changes once a month.
enum VoicePickerStore {
  /// Must match `DatabaseLocation.appGroup`, `plugins/withAppGroup.js` and the
  /// copy in `ios-app/AperioShortcuts.swift`.
  static let appGroup = "group.com.aperio.mobile"
  /// The intent side hardcodes this same name — the two targets share no source.
  static let fileName = "pickers.json"

  enum StoreError: Error {
    case noContainer
  }

  /// Replace the picker list.
  ///
  /// Scratch file and atomic swap, for the same reason as the widget snapshot:
  /// Siri can ask at any moment, and a plain overwrite has a window in which the
  /// file is a valid path holding half a document — which would present itself
  /// as "you have no calendars".
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
    _ = try FileManager.default.replaceItemAt(target, withItemAt: scratch)
  }
}
