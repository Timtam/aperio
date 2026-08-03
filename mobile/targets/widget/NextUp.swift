import SwiftUI
import WidgetKit

// "Nächster Termin" — the lock-screen widget: ONE row, with how long until it.
//
// A separate widget rather than a mode of the list one. A widget with modes is
// harder to grasp through a screen reader than two widgets with one purpose
// each, and the lock screen has room for exactly one line anyway.
//
// Lock-screen families only. The home screen already has the list; putting the
// same thing there twice would only make the gallery harder to choose from.

struct NextUpEntry: TimelineEntry {
    let date: Date
    /// The single row, or nil when there is nothing (or nothing known).
    let item: WidgetItem?
    let exhausted: Bool
    let strings: WidgetStrings
}

struct NextUpProvider: TimelineProvider {
    func placeholder(in context: Context) -> NextUpEntry {
        NextUpEntry(date: Date(), item: nil, exhausted: false, strings: fallbackStrings)
    }

    func getSnapshot(in context: Context, completion: @escaping (NextUpEntry) -> Void) {
        completion(entry(from: SnapshotLoader.load(), at: Date()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<NextUpEntry>) -> Void) {
        let now = Date()
        let snapshot = SnapshotLoader.load()
        let current = entry(from: snapshot, at: now)
        // The moments this widget's ONE row changes: when the current row
        // expires (the next one moves up) and, for a running event, when it
        // started — because the wording flips from a countdown to "läuft bis".
        // Far fewer entries than the list widget needs, which matters: a lock
        // screen redraw is the most frequently rendered thing we ship.
        var moments: Set<Date> = []
        for item in snapshot?.items(after: now) ?? [] {
            if let at = parseInstant(item.at), at > now { moments.insert(at) }
            if let expiry = item.expiresAt, expiry > now { moments.insert(expiry) }
        }
        let entries = [current] + moments.sorted().prefix(24).map { entry(from: snapshot, at: $0) }
        completion(Timeline(entries: entries, policy: .atEnd))
    }

    private func entry(from snapshot: WidgetSnapshot?, at date: Date) -> NextUpEntry {
        guard let snapshot else {
            return NextUpEntry(date: date, item: nil, exhausted: true, strings: fallbackStrings)
        }
        return NextUpEntry(
            date: date,
            item: snapshot.items(after: date).first,
            exhausted: snapshot.isExhausted(at: date),
            strings: snapshot.strings
        )
    }
}

/// How long until `date`, spelled out ("in 25 Minuten").
///
/// Formatted by the SYSTEM, in the device's language — not by us in the app's.
/// The two can differ, and this is a deliberate exception on the same grounds as
/// clock times: it is temporal formatting, with plural rules for every language
/// iOS ships, and hand-rolling it for two would be worse in both of them.
private func relativeText(to date: Date, from now: Date) -> String {
    let formatter = RelativeDateTimeFormatter()
    formatter.unitsStyle = .full
    return formatter.localizedString(for: date, relativeTo: now)
}

struct NextUpView: View {
    var entry: NextUpEntry
    @Environment(\.widgetFamily) private var family

    /// The second line: a countdown, or — once it has started — how long the
    /// event still runs. A bare countdown would go negative and read as nonsense
    /// at exactly the moment the row matters most.
    private func detail(for item: WidgetItem) -> String {
        guard let start = parseInstant(item.at) else { return "" }
        if item.untimed {
            // Nothing to count down to; the day is the whole answer.
            return Calendar.current.isDateInToday(start)
                ? entry.strings.today
                : (dayText(item.at) ?? "")
        }
        if start <= entry.date, let end = item.end.flatMap(parseInstant), end > entry.date {
            return entry.strings.runningUntil.replacingOccurrences(
                of: "{time}", with: timeText(item.end ?? "")
            )
        }
        return relativeText(to: start, from: entry.date)
    }

    /// One sentence for the whole widget. On a lock screen there is nothing
    /// around it to give it context, so it has to be complete on its own.
    private var spokenLabel: String {
        guard let item = entry.item else {
            return entry.exhausted ? entry.strings.stale : entry.strings.empty
        }
        return "\(item.title), \(detail(for: item)), \(kindWord(item, entry.strings))"
    }

    var body: some View {
        Group {
            if let item = entry.item {
                if family == .accessoryInline {
                    // One system-styled line, no layout of our own.
                    // `verbatim`: a Text built from a string LITERAL with
                    // interpolation is a LocalizedStringKey, and would be
                    // looked up in a table this target does not have.
                    Text(verbatim: "\(item.title) · \(detail(for: item))")
                } else {
                    VStack(alignment: .leading, spacing: 1) {
                        Label {
                            Text(item.title)
                        } icon: {
                            Image(systemName: kindSymbol(item))
                        }
                        .font(.headline)
                        .lineLimit(1)
                        Text(detail(for: item))
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                Text(entry.exhausted ? entry.strings.stale : entry.strings.empty)
                    .lineLimit(2)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spokenLabel)
    }
}

struct NextUpWidget: Widget {
    let kind = "AperioNextUp"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: NextUpProvider()) { entry in
            NextUpView(entry: entry)
                // Lock-screen accessories are drawn in the system's own material;
                // a fill of ours would fight it.
                .containerBackground(.clear, for: .widget)
        }
        .configurationDisplayName(galleryLanguageIsGerman ? "Nächster Termin" : "Next Up")
        .description(
            galleryLanguageIsGerman
                ? "Was als Nächstes ansteht, mit Countdown."
                : "What is next, with a countdown."
        )
        // Lock screen only. `.accessoryCircular` is left out on purpose: it can
        // hold a glyph or a number, neither of which can say WHAT is next, and a
        // widget that shows a time without its subject is a riddle.
        .supportedFamilies([.accessoryRectangular, .accessoryInline])
    }
}
