import Foundation

// The widget's side of the handover. The app derives what is next and leaves it
// in the App Group as JSON; nothing here computes, queries, or opens a database
// — an extension has neither the memory budget nor the access for that.
//
// These constants are duplicated from `modules/cal-ffi/ios/WidgetSnapshotStore`
// and `plugins/withAppGroup.js` because the app and the extension are separate
// targets that share no source. A mismatch fails SILENTLY: the container
// resolves to a different (empty) one, and the widget simply shows no data.

let appGroup = "group.com.aperio.mobile"
let snapshotFileName = "upcoming.json"

/// The shape `shared/widgetSnapshot.ts` writes. Kept deliberately flat so the
/// decoder cannot fail on a field the widget does not use.
struct WidgetSnapshot: Decodable {
    let version: Int
    let generatedAt: String
    let horizonEnd: String
    let strings: WidgetStrings
    let items: [WidgetItem]
}

/// The few words the widget has to say that are not data. They arrive with the
/// snapshot because the language is the one picked IN THE APP, which an
/// extension cannot read.
struct WidgetStrings: Decodable {
    let empty: String
    let stale: String
    let allDay: String
    let today: String
}

struct WidgetItem: Decodable {
    let kind: String
    let id: String
    let title: String
    let at: String
    let end: String?
    let untimed: Bool
    let containerId: String
    let color: String?
}

/// The version this build understands. A snapshot from a newer app is refused
/// rather than half-read — the app updates as one unit, but only its own process
/// restarts promptly, so a mismatch is a real (if brief) state.
let supportedSnapshotVersion = 1

enum SnapshotLoader {
    /// The snapshot on disk, or nil when there is none, it cannot be read, or it
    /// comes from a version this build does not know.
    static func load() -> WidgetSnapshot? {
        guard
            let container = FileManager.default
                .containerURL(forSecurityApplicationGroupIdentifier: appGroup),
            let data = try? Data(contentsOf: container.appendingPathComponent(snapshotFileName)),
            let snapshot = try? JSONDecoder().decode(WidgetSnapshot.self, from: data),
            snapshot.version == supportedSnapshotVersion
        else {
            return nil
        }
        return snapshot
    }
}

private let isoParser: ISO8601DateFormatter = {
    let f = ISO8601DateFormatter()
    // The app writes `toISOString()`, which always carries milliseconds.
    f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return f
}()

/// Parse an instant the app wrote. Falls back to the second-precision form so a
/// hand-written or re-serialised value still lands.
func parseInstant(_ raw: String) -> Date? {
    if let date = isoParser.date(from: raw) { return date }
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    return plain.date(from: raw)
}

extension WidgetItem {
    /// The moment this row stops being "next".
    ///
    /// Three cases, and getting the third wrong empties the widget of exactly
    /// the rows it exists for:
    ///   - an event: when it ENDS (a meeting in progress is the most relevant
    ///     row there is);
    ///   - an UNTIMED item with no end — a task due today, no clock time: its
    ///     instant is local MIDNIGHT, which is already in the past for all but
    ///     the first minute of the day. Taking that at face value would drop
    ///     every undated-today task the moment the widget rendered. It stands
    ///     until the day turns;
    ///   - anything else: when it starts.
    var expiresAt: Date? {
        if let end = end.flatMap(parseInstant) { return end }
        guard let at = parseInstant(self.at) else { return nil }
        guard untimed else { return at }
        return Calendar.current.date(byAdding: .day, value: 1, to: Calendar.current.startOfDay(for: at))
    }
}

extension WidgetSnapshot {
    /// Items that have not passed `date`, soonest first.
    ///
    /// The widget renders long after the app wrote this, so filtering by the
    /// RENDER time — not the write time — is what makes one snapshot serve a
    /// whole day of timeline entries.
    func items(after date: Date) -> [WidgetItem] {
        items.filter { item in
            guard let expiry = item.expiresAt else { return false }
            return expiry > date
        }
    }

    /// True once the covered window has run out, so an empty list means "nothing
    /// KNOWN" rather than "nothing planned" — a distinction a blind user cannot
    /// recover from a blank widget.
    func isExhausted(at date: Date) -> Bool {
        guard let horizon = parseInstant(horizonEnd) else { return true }
        return date >= horizon
    }
}
