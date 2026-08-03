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
    /// Aperio's language paired with the phone's region — see `localeFor`.
    let locale: Locale
}

struct UpcomingProvider: TimelineProvider {
    func placeholder(in context: Context) -> UpcomingEntry {
        // Empty rather than invented rows: in the gallery, plausible-looking
        // sample appointments are read out as if they were the user's own.
        UpcomingEntry(
            date: Date(), items: [], exhausted: false, strings: fallbackStrings,
            locale: Locale.current
        )
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
            // No snapshot means no language either; the device is all there is.
            return UpcomingEntry(
                date: date, items: [], exhausted: true, strings: fallbackStrings,
                locale: Locale.current
            )
        }
        return UpcomingEntry(
            date: date,
            items: snapshot.items(after: date),
            exhausted: snapshot.isExhausted(at: date),
            strings: snapshot.strings,
            locale: snapshot.resolvedLocale
        )
    }
}

/// The tick-off control's look: a circle that fills when checked, exactly the
/// shape every to-do list on the platform uses.
///
/// A custom style rather than the default switch. The interaction is bound by
/// `Toggle(isOn:intent:)` itself, not by the style, so a style that draws and
/// adds no gestures of its own leaves the behaviour untouched.
struct ChecklistToggleStyle: ToggleStyle {
    let tint: Color
    /// The unchecked glyph — an empty circle for an open task, a half-filled one
    /// for a task already underway. The state is drawn as well as spoken; a
    /// sighted user has no VoiceOver to tell them which is which.
    let unchecked: String

    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            Image(systemName: configuration.isOn ? "checkmark.circle.fill" : unchecked)
                .font(.caption)
                .foregroundStyle(tint)
            configuration.label
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

struct ItemRow: View {
    let item: WidgetItem
    let strings: WidgetStrings
    let locale: Locale
    /// Lock-screen rendering: everything on ONE line.
    ///
    /// `.accessoryRectangular` is about three lines of text in total, so a row
    /// that spends two of them on itself leaves room for one other row. Only the
    /// drawing gets tighter — the row keeps its checkbox and its spoken label is
    /// unchanged, so the lock screen reads and behaves like the home screen.
    var compact = false

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
        if let day = dayText(item.at, locale) {
            parts.append(day)
        } else if item.untimed && !isEvent(item) {
            parts.append(strings.today)
        }
        if !item.untimed {
            parts.append(timeText(item.at, locale))
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

    private var tint: Color {
        item.color.flatMap { Color(hex: $0) } ?? .secondary
    }

    var body: some View {
        if isCompletable(item) {
            // The ROW is the checkbox — one element, not a label plus a button
            // beside it. VoiceOver announces it with the checkbox trait and its
            // state, so what it is and what can be done with it arrive together,
            // in one stop instead of two.
            //
            // Always `isOn: false`: a completed task is not in the snapshot, and
            // one just ticked is hidden by the pending-action overlay. There is
            // no state here to get out of step with the app.
            Toggle(
                isOn: false,
                intent: CompleteTaskIntent(itemId: item.id, containerId: item.containerId)
            ) {
                label
            }
            .toggleStyle(ChecklistToggleStyle(tint: tint, unchecked: kindSymbol(item)))
            // One sentence instead of the two Texts inside, which VoiceOver
            // would otherwise read as fragments before the trait.
            .accessibilityLabel(spokenLabel)
        } else {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                // An event cannot be ticked, so it keeps a plain glyph where a
                // task has its circle — the sighted half of the word the label
                // speaks, tinted with the item's colour. Hidden from VoiceOver
                // because the label already says it in words.
                Image(systemName: kindSymbol(item))
                    .font(.caption2)
                    .foregroundStyle(tint)
                    .accessibilityHidden(true)
                label
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(spokenLabel)
        }
    }

    /// The row's text, in whichever density this family calls for.
    @ViewBuilder private var label: some View {
        if compact {
            Text(([item.title] + whenParts).joined(separator: " · "))
                .font(.caption)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            VStack(alignment: .leading, spacing: 1) {
                Text(item.title)
                    .font(.caption)
                    .lineLimit(1)
                Text(whenParts.joined(separator: " · "))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct UpcomingWidgetView: View {
    var entry: UpcomingEntry
    @Environment(\.widgetFamily) private var family

    /// Lock-screen placement. Narrow, monochrome, and drawn in the system's own
    /// material — so no background of ours and no buttons.
    private var isAccessory: Bool { family == .accessoryRectangular }

    /// Rows that fit. Anything past this is still IN the snapshot, driving the
    /// timeline as the day advances — it is the drawing that is capped, not the
    /// data.
    private var visible: [WidgetItem] {
        let count: Int
        switch family {
        // Three, which is what Apple's own rectangular accessory holds — its
        // documented example is "the top three to-dos". Each row is one line
        // here, so they fit.
        case .accessoryRectangular: count = 3
        case .systemSmall: count = 3
        default: count = 5
        }
        return Array(entry.items.prefix(count))
    }

    var body: some View {
        // Tighter on the lock screen: the rectangular accessory is roughly two
        // lines tall in total, so 4pt between rows is a row's worth of space.
        VStack(alignment: .leading, spacing: isAccessory ? 1 : 4) {
            if visible.isEmpty {
                // "Nothing planned" and "I have no current data" are different
                // facts and must never render the same way.
                Text(entry.exhausted ? entry.strings.stale : entry.strings.empty)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            } else {
                ForEach(visible, id: \.id) { item in
                    ItemRow(
                        item: item, strings: entry.strings, locale: entry.locale,
                        compact: isAccessory
                    )
                }
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // Applied HERE rather than in the configuration, because it depends on
        // the family: a lock-screen accessory is drawn in the system's own
        // material and a fill of ours would fight it.
        .containerBackground(for: .widget) {
            if !isAccessory {
                Rectangle().fill(.fill.tertiary)
            }
        }
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
        // Home screen AND lock screen. The rectangular accessory holds two
        // one-line rows — fewer than the home screen, but a list all the same,
        // which is what the lock screen was missing next to the single-row
        // countdown widget.
        .supportedFamilies([.systemSmall, .systemMedium, .accessoryRectangular])
    }
}

@main
struct AperioWidgetBundle: WidgetBundle {
    var body: some Widget {
        UpcomingWidget()
        NextUpWidget()
    }
}
