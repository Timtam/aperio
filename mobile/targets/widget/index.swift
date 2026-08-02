import SwiftUI
import WidgetKit

// Aperio's widget extension — step 1: it renders, it signs, it installs.
//
// Nothing here reads data. The database still lives in the app's sandbox,
// where an extension cannot reach it; moving it into the App Group container
// and giving cal-ffi a lean read path is step 2. What this build answers is
// only whether the target is generated and signed correctly at all, because
// that question costs a 30-minute EAS round trip and nothing further should be
// stacked on an unverified answer.

/// One entry, no data, no refresh.
///
/// A real provider will hand WidgetKit a timeline of upcoming items; this one
/// returns a single entry with a distant refresh date, because there is nothing
/// yet that could change.
struct PlaceholderEntry: TimelineEntry {
    let date: Date
}

struct PlaceholderProvider: TimelineProvider {
    func placeholder(in context: Context) -> PlaceholderEntry {
        PlaceholderEntry(date: Date())
    }

    func getSnapshot(in context: Context, completion: @escaping (PlaceholderEntry) -> Void) {
        completion(PlaceholderEntry(date: Date()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<PlaceholderEntry>) -> Void) {
        // `.never`: there is no data behind this yet, so asking WidgetKit to
        // come back would spend refresh budget on redrawing the same words.
        completion(Timeline(entries: [PlaceholderEntry(date: Date())], policy: .never))
    }
}

struct UpcomingWidgetView: View {
    var entry: PlaceholderEntry

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Aperio")
                .font(.headline)
            Text("Termine und Aufgaben folgen.")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        // One label for the whole widget rather than two elements a VoiceOver
        // user has to swipe between. A widget has no heading structure and no
        // context around it, so each addressable thing has to be a complete
        // sentence on its own.
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Aperio. Termine und Aufgaben folgen.")
    }
}

struct UpcomingWidget: Widget {
    let kind = "AperioUpcoming"

    var body: some WidgetConfiguration {
        StaticConfiguration(kind: kind, provider: PlaceholderProvider()) { entry in
            UpcomingWidgetView(entry: entry)
                .containerBackground(.fill.tertiary, for: .widget)
        }
        // Both strings are read out in the widget gallery, which is where a
        // screen-reader user picks this. "Aperio" alone would be indistinguishable
        // from the app's other widgets once there are more than one.
        .configurationDisplayName("Als Nächstes")
        .description("Die nächsten Termine und fälligen Aufgaben.")
        .supportedFamilies([.systemSmall, .systemMedium])
    }
}

@main
struct AperioWidgetBundle: WidgetBundle {
    var body: some Widget {
        UpcomingWidget()
    }
}
