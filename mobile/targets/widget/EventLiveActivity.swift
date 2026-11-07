import ActivityKit
import SwiftUI
import WidgetKit

// The Live Activity of an appointment (DESIGN-live-activities.md, "iOS"):
// from its last reminder until it ends, its time, its place and what comes
// next. The app decides every word and time (`shared/liveActivityIos.ts`);
// this only draws.
//
// Nothing here may say a relative time as fixed text: an activity is drawn
// once and then archived, so "in 10 minutes" would stay there. Relative times
// are the system's own self-updating text, absolute times stay true by
// themselves. iOS switches the content by itself exactly once, at the stale
// date: to the next appointment of the chain, or to "over".

struct EventLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: AperioEventAttributes.self) { context in
            EventActivityLockScreen(state: context.state, isStale: context.isStale)
                .padding(16)
                .widgetURL(tapURL(context.state, isStale: context.isStale))
        } dynamicIsland: { context in
            let view = ActivityView(state: context.state, isStale: context.isStale)
            return DynamicIsland {
                // Everything that matters in the centre and below: VoiceOver
                // focus is misplaced in the leading and trailing regions of the
                // expanded island, a bug Apple has confirmed.
                DynamicIslandExpandedRegion(.center) {
                    Text(view.shown.title)
                        .font(.headline)
                        .lineLimit(2)
                }
                DynamicIslandExpandedRegion(.bottom) {
                    EventActivityDetails(view: view)
                }
            } compactLeading: {
                Image(systemName: "calendar")
                    .accessibilityHidden(true)
            } compactTrailing: {
                // A start time stays true without an update; a countdown would
                // stop at zero once the appointment began.
                Text(timeText(view.shown.start, view.locale))
                    .accessibilityLabel(view.compactLabel)
            } minimal: {
                Text(timeText(view.shown.start, view.locale))
                    .accessibilityLabel(view.compactLabel)
            }
            .widgetURL(tapURL(context.state, isStale: context.isStale))
        }
    }
}

/// What a state shows, once the one switch iOS makes alone is taken into
/// account.
struct ActivityView {
    let state: AperioEventAttributes.ContentState
    let isStale: Bool

    /// The appointment on screen: the next one once stale, if there is one.
    var shown: AperioEventAttributes.Shown {
        isStale ? (state.next ?? state.current) : state.current
    }

    /// Stale with nothing after it: the appointment is over.
    var over: Bool { isStale && state.next == nil }

    var locale: Locale { localeFor(state.locale) }

    /// "Standup, 10:00" — the compact island's one short sentence.
    var compactLabel: String {
        "\(shown.title), \(timeText(shown.start, locale))"
    }

    /// "Danach: Review um 11:00", or nil.
    var followUp: String? {
        guard let title = shown.followUpTitle, let start = shown.followUpStart else { return nil }
        return state.words.followUp
            .replacingOccurrences(of: "{title}", with: title)
            .replacingOccurrences(of: "{time}", with: timeText(start, locale))
    }
}

struct EventActivityLockScreen: View {
    let state: AperioEventAttributes.ContentState
    let isStale: Bool

    var body: some View {
        let view = ActivityView(state: state, isStale: isStale)
        VStack(alignment: .leading, spacing: 4) {
            Text(view.shown.title)
                .font(.headline)
                .lineLimit(2)
            EventActivityDetails(view: view)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        // One element that reads as one sentence, the live times included.
        .accessibilityElement(children: .combine)
    }
}

struct EventActivityDetails: View {
    let view: ActivityView

    var body: some View {
        let shown = view.shown
        VStack(alignment: .leading, spacing: 2) {
            // "10:00 – 11:10": exact, and true without an update.
            Text("\(timeText(shown.start, view.locale)) – \(timeText(shown.end, view.locale))")
            if view.over {
                Text(view.state.words.over)
            } else {
                RelativeLine(word: view.state.words.begins, raw: shown.start, locale: view.locale)
                RelativeLine(word: view.state.words.ends, raw: shown.end, locale: view.locale)
            }
            if let location = shown.location, !location.isEmpty {
                Text(location)
            }
            if let followUp = view.followUp {
                Text(followUp)
            }
        }
        .font(.subheadline)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

/// "Beginn in 10 Minuten" / "Beginn vor 5 Minuten", kept up to date by the
/// system. Before iOS 18 there is no such text; the exact times above then
/// stand alone.
struct RelativeLine: View {
    let word: String
    let raw: String
    let locale: Locale

    var body: some View {
        if #available(iOS 18.0, *) {
            if let date = parseInstant(raw) {
                (Text(word) + Text(" ")
                    + Text(
                        .currentDate,
                        format: .reference(to: date, allowedFields: [.day, .hour, .minute])
                    ))
                    .environment(\.locale, locale)
            }
        }
    }
}

/// What a tap opens: the appointment on screen (`ReminderTapRouter`, read by
/// `reminderTapOfUrl` in shared/reminderTap.ts).
///
/// Every value is encoded down to letters and digits. `URLQueryItem` leaves
/// `&` and `=` alone, so a title like "Q&A" would split the query, and a `+`
/// in an instant's offset reads as a space to some decoders.
func tapURL(_ state: AperioEventAttributes.ContentState, isStale: Bool) -> URL? {
    let shown = ActivityView(state: state, isStale: isStale).shown
    func encoded(_ value: String) -> String {
        value.addingPercentEncoding(withAllowedCharacters: .alphanumerics) ?? ""
    }
    let query = [
        "eventId=\(encoded(shown.eventId))",
        "calendarId=\(encoded(shown.calendarId))",
        "start=\(encoded(shown.start))",
        "title=\(encoded(shown.title))",
    ].joined(separator: "&")
    return URL(string: "aperio://event?\(query)")
}
