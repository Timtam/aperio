import Foundation

// Wording and formatting shared by both widgets.
//
// These deliberately live outside either widget file: Swift's file-scope
// `private` is file-private, so anything both providers need has to sit
// somewhere neither of them owns.

/// The device's preferred language, the ONLY language signal available before a
/// snapshot exists. Used for the widget-gallery entries and the no-data
/// fallback; everything after that comes from the snapshot, in the language
/// picked inside the app.
var galleryLanguageIsGerman: Bool {
    (Locale.preferredLanguages.first ?? "en").hasPrefix("de")
}

/// Used when there is no snapshot at all — the gallery preview, and the window
/// between installing a widget and the app first running. It has to say
/// something true in a language nobody has told us yet.
var fallbackStrings: WidgetStrings {
    galleryLanguageIsGerman
        ? WidgetStrings(
            empty: "Nichts geplant.",
            noTimed: "Nichts mit Uhrzeit.",
            stale: "Keine aktuellen Daten. Öffne Aperio.",
            allDay: "Ganztägig",
            today: "Heute",
            runningUntil: "Läuft bis {time}",
            kindEvent: "Termin",
            kindTask: "Aufgabe"
        )
        : WidgetStrings(
            empty: "Nothing planned.",
            noTimed: "Nothing with a time.",
            stale: "No current data. Open Aperio.",
            allDay: "All day",
            today: "Today",
            runningUntil: "Running until {time}",
            kindEvent: "Event",
            kindTask: "Task"
        )
}

/// A time in the phone's regional format. Deliberately NOT translated by us:
/// times follow the device's regional settings like every other clock on the
/// home screen, while the words around them follow the app's language.
func timeText(_ raw: String) -> String {
    guard let date = parseInstant(raw) else { return "" }
    return date.formatted(date: .omitted, time: .shortened)
}

/// A spelled-out day ("Mi., 5. Aug."), or nil for today.
func dayText(_ raw: String) -> String? {
    guard let date = parseInstant(raw), !Calendar.current.isDateInToday(date) else { return nil }
    return date.formatted(.dateTime.weekday(.abbreviated).day().month(.abbreviated))
}

/// True for an event row. The wire format spells the kind as a string so a
/// future kind does not break older widgets on decode.
func isEvent(_ item: WidgetItem) -> Bool { item.kind == "event" }

/// The word for what this row IS, spoken at the end of its label.
func kindWord(_ item: WidgetItem, _ strings: WidgetStrings) -> String {
    isEvent(item) ? strings.kindEvent : strings.kindTask
}

/// The matching SF Symbol — the sighted half of the same information. Paired
/// with the word above rather than replacing it: an icon alone is exactly the
/// kind of meaning-by-picture a screen reader cannot recover.
func kindSymbol(_ item: WidgetItem) -> String {
    isEvent(item) ? "calendar" : "checkmark.circle"
}
