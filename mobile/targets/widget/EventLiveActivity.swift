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
//
// Apple caps a Live Activity at 160 points: the place and the follow-up keep
// to one line each, the two relative times share one, so the follow-up is
// not the line that gets cut. VoiceOver reads truncated text whole.

struct EventLiveActivity: Widget {
    var body: some WidgetConfiguration {
        ActivityConfiguration(for: AperioEventAttributes.self) { context in
            EventActivitySentence(
                view: ActivityView(state: context.state, isStale: context.isStale),
                titleLines: 2
            )
            .padding(14)
            .widgetURL(tapURL(context.state, isStale: context.isStale))
        } dynamicIsland: { context in
            let view = ActivityView(state: context.state, isStale: context.isStale)
            return DynamicIsland {
                // One region holds it all, so VoiceOver meets one sentence that
                // names the appointment; the leading and trailing regions are
                // also where Apple's confirmed focus bug misplaces it.
                DynamicIslandExpandedRegion(.bottom) {
                    EventActivitySentence(view: view, titleLines: 1)
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

/// The whole activity as one element that reads as one sentence: title,
/// "10:00 – 11:10", the live times, place, follow-up.
struct EventActivitySentence: View {
    let view: ActivityView
    let titleLines: Int

    var body: some View {
        let shown = view.shown
        // Hidden on the locked phone when notification previews are.
        let hide = view.state.hideOnLockScreen
        VStack(alignment: .leading, spacing: 2) {
            Text(shown.title)
                .font(.headline)
                .lineLimit(titleLines)
                .privacySensitive(hide)
            Text("\(timeText(shown.start, view.locale)) – \(timeText(shown.end, view.locale))")
                .font(.subheadline)
            if view.over {
                Text(view.state.words.over)
                    .font(.subheadline)
            } else {
                RelativeTimes(
                    words: view.state.words,
                    start: shown.start,
                    end: shown.end,
                    locale: view.locale
                )
            }
            if let location = shown.location, !location.isEmpty {
                Text(location)
                    .font(.subheadline)
                    .lineLimit(1)
                    .privacySensitive(hide)
            }
            if let followUp = view.followUp {
                Text(followUp)
                    .font(.subheadline)
                    .lineLimit(1)
                    .privacySensitive(hide)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
    }
}

/// "Beginn in 10 Minuten · Ende in 1 Stunde", kept up to date by the system,
/// on one line. Before iOS 18 there is no such text; the exact times above
/// then stand alone.
struct RelativeTimes: View {
    let words: AperioEventAttributes.Words
    let start: String
    let end: String
    let locale: Locale

    var body: some View {
        if #available(iOS 18.0, *) {
            if let from = parseInstant(start), let until = parseInstant(end) {
                (relative(words.begins, from) + Text(" · ") + relative(words.ends, until))
                    .font(.subheadline)
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                    .environment(\.locale, locale)
            }
        }
    }

    /// "Beginn in 10 Minuten": a word and the system's live relative time.
    /// Built in typed steps: one long `+` chain of `Text` can exhaust the type
    /// checker, and this file is first compiled on a build machine.
    @available(iOS 18.0, *)
    private func relative(_ word: String, _ date: Date) -> Text {
        let live: Text = Text(
            .currentDate,
            format: .reference(to: date, allowedFields: [.day, .hour, .minute])
        )
        let label: Text = Text(word) + Text(" ")
        return label + live
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
