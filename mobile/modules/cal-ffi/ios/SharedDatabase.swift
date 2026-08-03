import Foundation

/// Where the database lives, and the one-time move that put it there.
///
/// It used to sit in the app's Application Support directory — inside the
/// sandbox, which is exactly the place a widget extension cannot reach. A
/// widget runs in its own process, so for it to read anything the file has to
/// be in a container both sides can open: the App Group.
///
/// This type owns that decision and the migration into it. Nothing else should
/// build the path.
enum SharedDatabase {
  /// Must match `plugins/withAppGroup.js` and the widget's
  /// `expo-target.config.js`. A mismatch is silent — the container simply
  /// resolves to nil, or to a different empty one.
  static let appGroup = "group.com.aperio.mobile"

  private static let fileName = "aperio.sqlite"
  /// SQLite in WAL mode keeps three files. Moving the main one alone leaves a
  /// write-ahead log the database no longer knows about, which is how a
  /// "successful" migration loses the most recent writes.
  private static let siblings = ["", "-wal", "-shm"]
  /// Written only once the copy has been proven openable. Its presence — not
  /// the presence of the database file — is what says "migrated"; see
  /// `resolvePath`.
  private static let markerName = "aperio.migrated"

  /// The path to open, after migrating if this is the first launch that can.
  ///
  /// Returns the OLD path unchanged whenever anything about the move is not
  /// certain — no container, a copy that failed, a database that would not
  /// open. There is no half-migrated state: either the shared copy is complete
  /// and proven openable, or the app goes on using the file it always used.
  static func resolvePath(open: (String) throws -> Void) -> String {
    let legacy = legacyPath()
    guard let shared = sharedPath() else {
      NSLog("[Aperio] no App Group container for \(appGroup); staying in the sandbox")
      return legacy
    }
    let fm = FileManager.default
    // The marker, and NOT the mere existence of the file, decides. Getting this
    // backwards costs data in both directions, and the two failures need
    // opposite answers:
    //
    //   killed mid-copy — a partial file in the container, the good one still
    //   in the sandbox. Trusting the file here would silently adopt a truncated
    //   database.
    //
    //   deleting the originals failed after a proven migration — both exist,
    //   the SHARED one is live and the sandbox copy is now stale. Copying again
    //   would overwrite good data with old data.
    //
    // A marker written between "proven openable" and "delete the originals"
    // tells the two apart; neither timestamps nor file sizes can.
    if fm.fileExists(atPath: markerPath()) {
      return shared
    }
    // Nothing to move: a fresh install, or an earlier migration whose marker
    // did not survive. Either way the container is the right home from here on.
    guard fm.fileExists(atPath: legacy) else {
      markMigrated()
      return shared
    }
    return migrate(from: legacy, to: shared, open: open) ? shared : legacy
  }

  /// Copy, verify, and only then remove the originals.
  ///
  /// The order is the whole safety argument. A move-then-verify would, on a
  /// failure, leave the user with a database in neither place; copying first
  /// means the worst outcome is a redundant copy in the container that the next
  /// launch overwrites.
  private static func migrate(
    from legacy: String, to shared: String, open: (String) throws -> Void
  ) -> Bool {
    let fm = FileManager.default
    // Clear the whole destination set FIRST, not each file just before its own
    // copy. An interrupted attempt can leave a `-wal` whose source no longer
    // exists; the copy loop skips those, and the leftover would then sit beside
    // a freshly copied database as a write-ahead log belonging to another one.
    cleanUp(siblings.map { shared + $0 })
    var copied: [String] = []
    for suffix in siblings {
      let src = legacy + suffix
      let dst = shared + suffix
      guard fm.fileExists(atPath: src) else { continue }
      do {
        try fm.copyItem(atPath: src, toPath: dst)
        copied.append(dst)
      } catch {
        NSLog("[Aperio] copying \(src) into the App Group failed: \(error)")
        cleanUp(copied)
        return false
      }
      // Opening the copy is NOT enough on its own: an empty file is a perfectly
      // valid SQLite database, so a truncated copy would open without a
      // complaint and present itself as an app with no data. Byte counts catch
      // exactly the case the open cannot.
      // Written as "the source size is readable AND the copy matches it", not
      // as `size(dst) == size(src)`: two unreadable files both yield nil, and
      // nil == nil is true — the one comparison that must never pass.
      guard let expected = size(of: src), size(of: dst) == expected else {
        NSLog("[Aperio] the copy of \(src) came out a different size; leaving everything where it is")
        cleanUp(copied)
        return false
      }
    }
    // Proven, not assumed: the copy has to actually open before the originals
    // are allowed to go. The size check above says the bytes arrived; only this
    // says they are a database this build can still use.
    do {
      try open(shared)
    } catch {
      NSLog("[Aperio] the migrated database did not open: \(error)")
      cleanUp(copied)
      return false
    }
    // Between the proof and the deletion — the only window in which both copies
    // are good, and therefore the only moment where the marker cannot lie.
    markMigrated()
    for suffix in siblings {
      let src = legacy + suffix
      guard fm.fileExists(atPath: src) else { continue }
      // A failure HERE is harmless and deliberately not fatal: the shared copy
      // is good and in use, and what stays behind is an orphan in the sandbox
      // that nothing reads. Undoing a proven-good migration over it would be
      // the worse trade.
      try? fm.removeItem(atPath: src)
    }
    NSLog("[Aperio] database migrated into \(appGroup)")
    return true
  }

  /// nil when the size cannot be read — which fails the comparison, as it
  /// should: an unreadable copy is not a verified one.
  private static func size(of path: String) -> Int64? {
    guard let attributes = try? FileManager.default.attributesOfItem(atPath: path) else {
      return nil
    }
    // `.size` is an NSNumber behind an Any. Going through `int64Value` rather
    // than `as? Int64` avoids depending on how that bridges.
    return (attributes[.size] as? NSNumber)?.int64Value
  }

  private static func cleanUp(_ paths: [String]) {
    for path in paths {
      try? FileManager.default.removeItem(atPath: path)
    }
  }

  private static func markerPath() -> String {
    guard let container = FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup)
    else {
      // Unreachable in practice: every caller has already established the
      // container. A path that matches nothing is the safe answer anyway — it
      // reads as "not migrated", which keeps the app on the legacy file.
      return ""
    }
    return container.appendingPathComponent(markerName).path
  }

  /// Failing to write it is deliberately not an error. The next launch then
  /// finds no marker and no legacy database, concludes the same thing, and
  /// tries again.
  private static func markMigrated() {
    let path = markerPath()
    guard !path.isEmpty else { return }
    if !FileManager.default.createFile(atPath: path, contents: Data()) {
      NSLog("[Aperio] could not write the migration marker; the next launch will re-check")
    }
  }

  /// The pre-migration location. Still read on a device whose container is
  /// unavailable, so it cannot simply be deleted from this file.
  private static func legacyPath() -> String {
    let dir = try! FileManager.default.url(
      for: .applicationSupportDirectory,
      in: .userDomainMask,
      appropriateFor: nil,
      create: true
    )
    return dir.appendingPathComponent(fileName).path
  }

  /// The shared container, or nil when the entitlement is missing — which is
  /// what a build signed without the App Group looks like from in here.
  static func sharedPath() -> String? {
    FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
      .appendingPathComponent(fileName)
      .path
  }
}
