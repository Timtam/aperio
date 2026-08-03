import SwiftUI
import WidgetKit

// Aperio's "Up Next" widget: the next events and due tasks, read from the
// snapshot the app leaves in the App Group (see Snapshot.swift for the shape and
// why the derivation happens on the app's side).
//
// Nothing here queries anything. The provider's whole job is to turn ONE
// snapshot into a series of moments — because WidgetKit renders on its own
// schedule, hours after this ran, and asks "what is next" at times nobody knew
// when the file was written.

struct UpcomingEntry: TimelineEntry {
    let date: Date
    /// What is next AT `date`, already filtered. Empty is a legitimate state.
    let items: [WidgetItem]
    /// The window ran out — an empty list no longer means "nothing planned".
    let exhausted: Bool
    let strings: WidgetStrings
}

struct UpcomingProvider: TimelineProvider {
    func placeholder(in context: Context) -> UpcomingEntry {
        // Empty rather than invented rows: in the gallery, plausible-looking
        // sample appointments are read out as if they were the user's own.
        UpcomingEntry(date: Date(), items: [], exhausted: false, strings: fallbackStrings)
    }

    func getSnapshot(in context: Context, completion: @escaping (UpcomingEntry) -> Void) {
        completion(entry(from: SnapshotLoader.load(), at: Date()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<UpcomingEntry>) -> Void) {
        let now = Date()
        // Read and decoded ONCE for the whole timeline. Doing it per entry would
        // re-open and re-parse the same file up to two dozen times inside a
        // process that is measured on how little it does.
        let snapshot = SnapshotLoader.load()
        let current = entry(from: snapshot, at: now)
        // One entry per moment the DISPLAY changes — each row's start and each
        // row's expiry — so a finished meeting disappears at the instant it
        // ends, without spending a refresh on anything else. Widgets get a small
        // daily budget of system-initiated reloads; a fixed interval would spend
        // it redrawing unchanged words and still be late for the change.
        var moments: Set<Date> = []
        for item in current.items {
            if let at = parseInstant(item.at), at > now { moments.insert(at) }
            if let expiry = item.expiresAt, expiry > now { moments.insert(expiry) }
        }
        // A cap: a busy week would otherwise produce dozens of entries, and
        // WidgetKit keeps only the first handful anyway.
        let entries =
            [current] + moments.sorted().prefix(24).map { entry(from: snapshot, at: $0) }
        // `.atEnd`: come back once the last known moment has passed. The app
        // reloads the timeline itself whenever the data changes, so this is only
        // the floor under a phone nobody has opened.
        completion(Timeline(entries: entries, policy: .atEnd))
    }

    private func entry(from snapshot: WidgetSnapshot?, at date: Date) -> UpcomingEntry {
        guard let snapshot else {
            // No snapshot is NOT an empty calendar, and must not read like one.
            return UpcomingEntry(date: date, items: [], exhausted: true, strings: fallbackStrings)
        }
        return UpcomingEntry(
            date: date,
            items: snapshot.items(after: date),
            exhausted: snapshot.isExhausted(at: date),
            strings: snapshot.strings
        )
    }
}

struct ItemRow: View {
    let item: WidgetItem
    let strings: WidgetStrings

    /// "When", in the order it reads: the day, then the time.
    ///
    /// Today's day label is normally left off — a bare time reads as today when
    /// the rows after it carry dates, the same convention the app's day view
    /// uses. The exception is a row with NO time: an untimed task due today
    /// would otherwise answer "when" with silence, so it says so in words.
    /// "Ganztägig" is reserved for all-day EVENTS, which is a property a task
    /// does not have.
    private var whenParts: [String] {
        var parts: [String] = []
        if let day = dayText(item.at) {
            parts.append(day)
        } else if item.untimed && !isEvent(item) {
            parts.append(strings.today)
        }
        if !item.untimed {
            parts.append(timeText(item.at))
        } else if isEvent(item) {
            parts.append(strings.allDay)
        }
        return parts.filter { !$0.isEmpty }
    }

    /// The whole row as one sentence. A widget has no headings and no
    /// surrounding context, so every addressable element has to stand alone —
    /// splitting this into a title element and a time element would make
    /// VoiceOver read two fragments, neither of which is an appointment.
    private var spokenLabel: String {
        ([item.title] + whenParts + [kindWord(item, strings)]).joined(separator: ", ")
    }

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            // What KIND of row this is, as a glyph — the sighted half of the
            // word the label speaks. Tinted with the item's colour when it has
            // one, so the two cues share the space a colour dot used to take
            // and neither is the only carrier of meaning.
            Image(systemName: kindSymbol(item))
                .font(.caption2)
                .foregroundStyle(item.color.flatMap { Color(hex: $0) } ?? .secondary)
            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .font(.caption)
                    .lineLimit(1)
                Text(whenParts.joined(separator: " · "))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(spokenLabel)
    }
}

struct UpcomingWidgetView: View {
    var entry: UpcomingEntry
    @Environment(\.widgetFamily) private var family

    /// Rows that fit. A small widget shows fewer than a medium one; anything
    /// past this is still IN the snapshot, driving the timeline as the day
    /// advances.
    private var visible: [WidgetItem] {
        Array(entry.items.prefix(family == .systemSmall ? 3 : 5))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if visible.isEmpty {
                // "Nothing planned" and "I have no current data" are different
                // facts and must never render the same way.
                Text(entry.exhausted ? entry.strings.stale : entry.strings.empty)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(visible, id: \.id) { item in
                    ItemRow(item: item, strings: entry.strings)
                }
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

extension Color {
    /// `#rrggbb` as the app writes it. Returns nil for anything else rather than
    /// guessing a colour.
    init?(hex: String) {
        var value = hex
        if value.hasPrefix("#") { value.removeFirst() }
        guard value.count == 6, let rgb = UInt32(value, radix: 16) else { return nil }
        self.init(
            .sRGB,
            red: Double((rgb >> 16) & 0xFF) / 255,
            green: Double((rgb >> 8) & 0xFF) / 255,
            blue: Double(rgb & 0xFF) / 255
        )
    }
}

struct UpcomingWidget: Widget {
    let kind = "AperioUpcoming"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: UpcomingProvider()) { entry in
            UpcomingWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        // Both strings are read out in the widget gallery, which is where a
        // screen-reader user picks this. "Aperio" alone would be
        // indistinguishable from the app's other widgets once there is more
        // than one.
        .configurationDisplayName(galleryLanguageIsGerman ? "Als Nächstes" : "Up Next")
        .description(
            galleryLanguageIsGerman
                ? "Die nächsten Termine und fälligen Aufgaben."
                : "Your next events and due tasks."
        )
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

@main
struct AperioWidgetBundle: WidgetBundle {
    var body: some Widget {
        UpcomingWidget()
        NextUpWidget()
    }
}
