import Foundation

/// Where the database lives — and the one-time move that brings it back home.
///
/// It spent one release inside the App Group container, so that a widget
/// extension could open it. Nothing needs that any more: the widgets read a
/// small JSON snapshot the app writes for them (see `WidgetSnapshotStore`) and
/// never touch SQLite at all.
///
/// Leaving it there was not merely redundant, it was fatal. iOS terminates an
/// app with `0xdead10cc` when it is suspended while holding a file lock on
/// anything in a SHARED container — and a WAL-mode SQLite connection holds
/// exactly such a lock for as long as it is open, which for us is the whole
/// lifetime of the process. Every suspension was a coin flip, and the crash
/// arrives with no stack of ours in it: the main thread is idle in its run
/// loop, because nothing went wrong in our code. The same connection in the
/// app's OWN sandbox is not watched at all.
///
/// So the sandbox is home, and this type owns the move back. Nothing else
/// should build the path.
enum DatabaseLocation {
  /// Still needed — the snapshot and the widget's action queue live in the
  /// container even though the database no longer does. Must match
  /// `plugins/withAppGroup.js` and the widget's `expo-target.config.js`.
  static let appGroup = "group.com.aperio.mobile"

  private static let fileName = "aperio.sqlite"
  /// SQLite in WAL mode keeps three files. Moving the main one alone leaves a
  /// write-ahead log the database no longer knows about, which is how a
  /// "successful" migration loses the most recent writes.
  private static let siblings = ["", "-wal", "-shm"]
  /// Written by the build that moved the database INTO the container, and
  /// deleted by the move back out. Its presence — not the presence of any
  /// file — is what says "the container copy is the live one"; see
  /// `resolvePath`.
  private static let markerName = "aperio.migrated"

  /// The path to open, after bringing the database home if an earlier build
  /// left it in the container.
  ///
  /// Returns the CONTAINER path unchanged whenever anything about the move is
  /// not certain — a copy that failed, a database that would not open. There is
  /// no half-migrated state: either the local copy is complete and proven
  /// openable, or the app goes on using the file it has been using.
  static func resolvePath(open: (String) throws -> Void) -> String {
    let local = localPath()
    guard let shared = sharedPath() else {
      // No container at all (a build signed without the App Group). Then the
      // database was never moved out, and there is nothing to bring back.
      return local
    }
    let fm = FileManager.default
    if fm.fileExists(atPath: markerPath()) {
      return migrate(from: shared, to: local, open: open) ? local : shared
    }
    // No marker. Almost always: this device never ran the build that moved the
    // database out, or it already moved it back. But the marker write was
    // best-effort in that build, so "no marker" is not by itself proof that the
    // local file is the live one — and adopting the wrong one costs everything
    // in either direction. The files themselves settle it:
    //
    //   local missing, container holding a database — only one of them can be
    //   the user's data. Bring it back, marker or no marker.
    //
    //   local present — then it is live, and whatever sits in the container is
    //   an orphan from a copy that was interrupted before it committed. Adopting
    //   THAT would overwrite good data with a partial or stale one.
    if !fm.fileExists(atPath: local), fm.fileExists(atPath: shared) {
      return migrate(from: shared, to: local, open: open) ? local : shared
    }
    cleanUp(siblings.map { shared + $0 })
    return local
  }

  /// Copy, verify, and only then let go of the container copy.
  ///
  /// The order is the whole safety argument. A move-then-verify would, on a
  /// failure, leave the user with a database in neither place; copying first
  /// means the worst outcome is a redundant copy the next launch overwrites.
  private static func migrate(
    from shared: String, to local: String, open: (String) throws -> Void
  ) -> Bool {
    let fm = FileManager.default
    // Nothing to bring back: a previous pass already did, and only the marker
    // outlived the cleanup. Clearing the destination below would delete the very
    // database we are trying to protect, so this guard comes FIRST.
    guard fm.fileExists(atPath: shared) else {
      unmark()
      return true
    }
    // Clear the whole destination set, not each file just before its own copy.
    // An interrupted attempt can leave a `-wal` whose source no longer exists;
    // the copy loop skips those, and the leftover would then sit beside a
    // freshly copied database as a write-ahead log belonging to another one.
    cleanUp(siblings.map { local + $0 })
    var copied: [String] = []
    for suffix in siblings {
      let src = shared + suffix
      let dst = local + suffix
      guard fm.fileExists(atPath: src) else { continue }
      do {
        try fm.copyItem(atPath: src, toPath: dst)
        copied.append(dst)
      } catch {
        NSLog("[Aperio] copying \(src) out of the App Group failed: \(error)")
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
    // Proven, not assumed: the copy has to actually open before the container
    // copy is allowed to go. The size check above says the bytes arrived; only
    // this says they are a database this build can still use.
    do {
      try open(local)
    } catch {
      NSLog("[Aperio] the recovered database did not open: \(error)")
      cleanUp(copied)
      return false
    }
    // The commit point, and the only moment at which the marker cannot lie:
    // both copies are good, and from here the LOCAL one is the live one.
    //
    // A failure to clear the marker is survivable — the next launch finds the
    // marker but no container database and takes the guard at the top — but a
    // failure to clear it AFTER deleting the container copy would not be, which
    // is why this comes before the deletion and not after.
    unmark()
    for suffix in siblings {
      // Harmless if it fails: the local copy is proven and in use, and what
      // stays behind is an orphan that the no-marker branch of `resolvePath`
      // sweeps on a later launch.
      try? fm.removeItem(atPath: shared + suffix)
    }
    NSLog("[Aperio] database brought back out of \(appGroup)")
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
      // A path that matches nothing reads as "not migrated", which keeps the app
      // on the local file — the safe answer.
      return ""
    }
    return container.appendingPathComponent(markerName).path
  }

  private static func unmark() {
    let path = markerPath()
    guard !path.isEmpty else { return }
    try? FileManager.default.removeItem(atPath: path)
  }

  /// Home. The path the app used before the container detour, and again after
  /// it — so an install that never ran that one build sees no change at all.
  private static func localPath() -> String {
    let dir = try! FileManager.default.url(
      for: .applicationSupportDirectory,
      in: .userDomainMask,
      appropriateFor: nil,
      create: true
    )
    return dir.appendingPathComponent(fileName).path
  }

  /// The container copy's path, or nil when the entitlement is missing — which
  /// is what a build signed without the App Group looks like from in here.
  static func sharedPath() -> String? {
    FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
      .appendingPathComponent(fileName)
      .path
  }
}
