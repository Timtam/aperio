import Foundation

// Lines from native code into Aperio's own log: the file the Logs screen shows
// and a feedback mail carries.
//
// `NSLog` reaches only the device's system log, which nobody can send us. The
// background wake-up's start, its expiry and what it ended were therefore
// invisible in the field: on Toni's iPhone (build 71, 2026-11-17) a round woke,
// lived eight seconds and left no line saying whether it ran out of time.
//
// The log belongs to the Host, which the module opens once the JS bundle has
// started; a launch into the background reaches the wake-up handler well before
// that. Lines written earlier wait here, with the moment they happened in the
// text, and go out when the module installs the sink. A process iOS kills
// before then loses them.
enum NativeLog {
  private static let lock = NSLock()
  private static var sink: ((String, String) -> Void)?
  private static var waiting: [(level: String, message: String)] = []
  /// A launch that never opens the Host must not grow this without bound.
  private static let maxWaiting = 50

  /// Called once the Host is open; writes what waited, in order.
  static func install(_ write: @escaping (String, String) -> Void) {
    lock.lock()
    sink = write
    let queued = waiting
    waiting = []
    lock.unlock()
    for line in queued {
      write(line.level, line.message)
    }
  }

  /// `level` is one of the log's levels (`info`, `warn`, ...).
  static func write(_ level: String, _ message: String) {
    lock.lock()
    if let write = sink {
      lock.unlock()
      write(level, message)
      return
    }
    if waiting.count < maxWaiting {
      let stamp = ISO8601DateFormatter().string(from: Date())
      waiting.append((level, "\(message) (at \(stamp))"))
    }
    lock.unlock()
  }
}
