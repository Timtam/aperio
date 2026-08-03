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

/// Aperio's LANGUAGE with the phone's REGION.
///
/// The two answer different questions and must not be taken from one source.
/// German or English is Aperio's own setting, which the user may have overridden
/// against their device. A 24-hour clock and day-before-month are the phone's
/// regional settings, which no app should override. Forcing both from one tag
/// fixes the words by breaking the numbers.
///
/// Why the tag has to be shipped at all: `Locale.current` inside an extension is
/// intersected with the localizations its BUNDLE declares, and a widget
/// extension with no `.lproj` folders declares none — so it falls back to the
/// development language and reads "in 17 hours" on a German phone.
func localeFor(_ tag: String) -> Locale {
    let language = tag.replacingOccurrences(of: "_", with: "-")
        .split(separator: "-").first.map(String.init) ?? tag
    guard let region = Locale.current.region?.identifier else {
        return Locale(identifier: language)
    }
    return Locale(identifier: "\(language)_\(region)")
}

/// A time, in the given locale's format.
func timeText(_ raw: String, _ locale: Locale) -> String {
    guard let date = parseInstant(raw) else { return "" }
    return date.formatted(Date.FormatStyle(date: .omitted, time: .shortened).locale(locale))
}

/// A spelled-out day ("Mi., 5. Aug."), or nil for today.
func dayText(_ raw: String, _ locale: Locale) -> String? {
    guard let date = parseInstant(raw), !Calendar.current.isDateInToday(date) else { return nil }
    return date.formatted(
        .dateTime.weekday(.abbreviated).day().month(.abbreviated).locale(locale)
    )
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

/// Whether the widget may offer a tick-off for this row. The app decides and
/// says so in the snapshot; this only reads the answer.
func isCompletable(_ item: WidgetItem) -> Bool { item.completable == true }
