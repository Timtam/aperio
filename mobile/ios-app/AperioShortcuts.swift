import AppIntents
import Foundation

// Aperio's Siri shortcuts.
//
// This file is NOT part of any Expo module — it is copied into the generated
// iOS app target by `plugins/withAppShortcuts.js`, and that is a requirement
// rather than a preference: an `AppShortcutsProvider` is only discovered in the
// MAIN app target, and Apple's framework-level escape hatch (`AppIntentsPackage`)
// covers frameworks only, not the static libraries Expo modules compile to.
//
// The intents do NOT create anything themselves. Same reason the widget's
// checkbox does not complete a task: creating one means the event log, the sync
// queue and the adapter rules, and an intent has no way to reach them. So each
// writes the REQUEST into the App Group, exactly like the widget's queue, and
// the app performs it on the way in.
//
// `openAppWhenRun` is the difference from the widget, and it is deliberate. A
// tap that vanishes a row can afford to be applied minutes later; a spoken "make
// an appointment tomorrow at eleven" cannot. You would say it, look, and find
// nothing. So the app comes forward, drains the queue and shows the result.
//
// LANGUAGE. Every phrase below is ENGLISH, and that is not an oversight — this
// file is the development language, and translations do not belong in it. Siri
// matches against the phrase set for the language IT is set to, and reads that
// set from `<lang>.lproj/AppShortcuts.strings` in the app bundle, keyed by the
// English phrase. A German phrase written as a literal here would be registered
// as an English one and never offered to a German Siri.
//
// Which is what happened: with only the literals, "erstelle einen neuen Termin
// mit Aperio" reached Apple's Calendar instead of us. So the German wording now
// lives in `ios-app/de.lproj/AppShortcuts.strings`, and the phrases come in
// several shapes per language, because a spoken sentence has to hit one of them
// almost exactly and nobody says the same thing twice.

/// The App Group where the app and its extensions leave things for each other.
/// Must match `plugins/withAppGroup.js`, `VoicePickerStore`, `WidgetSnapshotStore`
/// and the widget's own copy — a mismatch is silent in all of them.
private let appGroup = "group.com.aperio.mobile"
private let actionsDirectoryName = "actions"
private let pickersFileName = "pickers.json"

/// The id that means "let the app decide", as it always has: the last-used
/// calendar or list, falling back to the first writable one.
///
/// It exists so the picker can never be EMPTY. A required parameter whose query
/// returns nothing is a dead end — Siri asks a question with no possible answer
/// — and the list is genuinely empty on a phone where the app has not completed
/// a pass yet. A first entry that always works costs one line and removes that
/// failure entirely; it also gives anyone who does not care a one-word answer.
private let defaultPickId = "__default__"

// MARK: - The lists a voice request may name

/// Decoded from what the app leaves in `pickers.json`; see `VoicePickerStore`.
private struct PickerFile: Decodable {
  struct Entry: Decodable {
    let id: String
    let name: String
  }

  let calendars: [Entry]
  let taskLists: [Entry]
}

private func readPickerFile() -> PickerFile? {
  guard
    let container = FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup),
    let data = try? Data(contentsOf: container.appendingPathComponent(pickersFileName))
  else {
    return nil
  }
  return try? JSONDecoder().decode(PickerFile.self, from: data)
}

@available(iOS 16.0, *)
struct AperioCalendar: AppEntity {
  let id: String
  let name: String

  static var typeDisplayRepresentation = TypeDisplayRepresentation(name: "Calendar")
  static var defaultQuery = AperioCalendarQuery()

  /// The user's own calendar name, so it is NOT looked up in a strings file —
  /// `LocalizedStringResource(stringLiteral:)` passes it through verbatim.
  var displayRepresentation: DisplayRepresentation {
    DisplayRepresentation(title: LocalizedStringResource(stringLiteral: name))
  }

  static func all() -> [AperioCalendar] {
    let fromApp = readPickerFile()?.calendars.map { AperioCalendar(id: $0.id, name: $0.name) } ?? []
    return [AperioCalendar(id: defaultPickId, name: String(localized: "Default calendar"))] + fromApp
  }
}

/// `EntityStringQuery` rather than plain `EntityQuery`, because the interesting
/// case is exactly the one plain `EntityQuery` cannot serve: the user SAYS a
/// name and Siri has to turn it into an entity.
@available(iOS 16.0, *)
struct AperioCalendarQuery: EntityStringQuery {
  func entities(for identifiers: [String]) async throws -> [AperioCalendar] {
    AperioCalendar.all().filter { identifiers.contains($0.id) }
  }

  func entities(matching string: String) async throws -> [AperioCalendar] {
    AperioCalendar.all().filter { $0.name.localizedCaseInsensitiveContains(string) }
  }

  func suggestedEntities() async throws -> [AperioCalendar] {
    AperioCalendar.all()
  }
}

@available(iOS 16.0, *)
struct AperioTaskList: AppEntity {
  let id: String
  let name: String

  static var typeDisplayRepresentation = TypeDisplayRepresentation(name: "Task list")
  static var defaultQuery = AperioTaskListQuery()

  var displayRepresentation: DisplayRepresentation {
    DisplayRepresentation(title: LocalizedStringResource(stringLiteral: name))
  }

  static func all() -> [AperioTaskList] {
    let fromApp = readPickerFile()?.taskLists.map { AperioTaskList(id: $0.id, name: $0.name) } ?? []
    return [AperioTaskList(id: defaultPickId, name: String(localized: "Default list"))] + fromApp
  }
}

@available(iOS 16.0, *)
struct AperioTaskListQuery: EntityStringQuery {
  func entities(for identifiers: [String]) async throws -> [AperioTaskList] {
    AperioTaskList.all().filter { identifiers.contains($0.id) }
  }

  func entities(matching string: String) async throws -> [AperioTaskList] {
    AperioTaskList.all().filter { $0.name.localizedCaseInsensitiveContains(string) }
  }

  func suggestedEntities() async throws -> [AperioTaskList] {
    AperioTaskList.all()
  }
}

// MARK: - The queue

/// Queue one request for the app to carry out.
///
/// One file per request, never a shared one: the widget's extension and these
/// intents all write here, and a read-modify-write on a common file loses
/// whichever caller lost the race.
private func enqueue(_ payload: [String: Any]) {
  guard
    let container = FileManager.default
      .containerURL(forSecurityApplicationGroupIdentifier: appGroup)
  else {
    return
  }
  let directory = container.appendingPathComponent(actionsDirectoryName, isDirectory: true)
  try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
  guard let data = try? JSONSerialization.data(withJSONObject: payload) else { return }
  try? data.write(
    to: directory.appendingPathComponent("\(UUID().uuidString).json"),
    options: .atomic
  )
}

private let isoFormatter: ISO8601DateFormatter = {
  let f = ISO8601DateFormatter()
  f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
  f.timeZone = TimeZone(identifier: "UTC")
  return f
}()

/// The reserved id is a marker, not a container — sending it on would have the
/// app look for a calendar called `__default__`. Omitting the key entirely is
/// what the app already reads as "choose for me".
private func pickedId(_ id: String) -> String? {
  id == defaultPickId ? nil : id
}

// MARK: - Intents

@available(iOS 16.0, *)
struct OpenAperioIntent: AppIntent {
  static var title: LocalizedStringResource = "Open Aperio"
  static var description = IntentDescription("Opens Aperio.")
  /// The whole point of this one: if the app comes forward, the plumbing works.
  static var openAppWhenRun: Bool = true

  func perform() async throws -> some IntentResult {
    .result()
  }
}

@available(iOS 16.0, *)
struct CreateEventIntent: AppIntent {
  static var title: LocalizedStringResource = "New event"
  static var description = IntentDescription("Creates an event in Aperio.")
  /// Creating something has to be visible. See the note at the top of the file.
  static var openAppWhenRun: Bool = true

  @Parameter(title: "Title", requestValueDialog: "What should the event be called?")
  var eventTitle: String

  /// Siri resolves the spoken time itself — "tomorrow at eleven", "next
  /// Tuesday at two" — and hands over a real Date. This is why the whole
  /// feature needs no natural-language date parser of ours, and why it is iOS
  /// only: nothing on the other platform offers the same.
  ///
  /// It cannot appear in the shortcut PHRASE, though. Siri asks for it
  /// instead, which for a screen reader is arguably the better shape: each
  /// part is confirmed as it is given.
  @Parameter(title: "Date", requestValueDialog: "When?")
  var date: Date

  /// Asked for, not guessed. Before this existed the app used the last calendar
  /// touched in the EDITOR, which is a reasonable default for a button next to
  /// a calendar picker and a bad one for a sentence spoken across the room —
  /// the event kept landing somewhere the speaker had not chosen and could not
  /// see.
  @Parameter(title: "Calendar", requestValueDialog: "Which calendar?")
  var calendar: AperioCalendar

  func perform() async throws -> some IntentResult {
    var payload: [String: Any] = [
      "version": 1,
      "action": "createEvent",
      "title": eventTitle,
      "startsAt": isoFormatter.string(from: date),
      "at": isoFormatter.string(from: Date()),
    ]
    if let id = pickedId(calendar.id) { payload["calendarId"] = id }
    enqueue(payload)
    return .result()
  }
}

@available(iOS 16.0, *)
struct CreateTaskIntent: AppIntent {
  static var title: LocalizedStringResource = "New task"
  static var description = IntentDescription("Creates a task in Aperio.")
  static var openAppWhenRun: Bool = true

  @Parameter(title: "Title", requestValueDialog: "What should the task be called?")
  var taskTitle: String

  @Parameter(title: "Task list", requestValueDialog: "Which list?")
  var list: AperioTaskList

  /// Optional, and therefore NOT asked for — Siri only requests the parameters
  /// it must have. That is the right default: most spoken tasks are "remember
  /// this", and a task with no day belongs in the backlog rather than being
  /// forced onto one. It is still a real parameter, so a shortcut built by hand
  /// in the Shortcuts app can set it.
  ///
  /// Named for what it actually sets — the day the task is PLANNED for, which
  /// is what quick-add fills in too. Aperio keeps that separate from a
  /// deadline, and calling this one "due" would quietly write to the wrong
  /// field.
  @Parameter(title: "Scheduled date")
  var scheduledDate: Date?

  func perform() async throws -> some IntentResult {
    var payload: [String: Any] = [
      "version": 1,
      "action": "createTask",
      "title": taskTitle,
      "at": isoFormatter.string(from: Date()),
    ]
    if let id = pickedId(list.id) { payload["listId"] = id }
    if let day = scheduledDate { payload["scheduledAt"] = isoFormatter.string(from: day) }
    enqueue(payload)
    return .result()
  }
}

// MARK: - Phrases

@available(iOS 16.0, *)
struct AperioShortcuts: AppShortcutsProvider {
  static var appShortcuts: [AppShortcut] {
    AppShortcut(
      intent: OpenAperioIntent(),
      // Every phrase MUST contain `\(.applicationName)` — Siri refuses to
      // hand a bare verb to a third-party app, which is also why "create
      // an event" alone can never reach us. The build fails outright on a
      // phrase that leaves it out.
      phrases: [
        "Open \(.applicationName)"
      ],
      shortTitle: "Open Aperio",
      systemImageName: "calendar"
    )
    AppShortcut(
      intent: CreateEventIntent(),
      // No date in the phrase — Siri refuses a `Date` there, and a free
      // `String` is no better: a phrase parameter has to come from a
      // finite set (an `AppEnum` or an `AppEntity`). So the title and the
      // time are both asked for, and the phrase only has to get us the
      // handover.
      //
      // Which makes the RANGE of phrasings the whole game. Each one is a
      // separate way in, and the German translations in
      // `de.lproj/AppShortcuts.strings` are keyed by these exact strings —
      // change one here and the matching key must change with it.
      phrases: [
        "New event in \(.applicationName)",
        "Create an event in \(.applicationName)",
        "Create a new event in \(.applicationName)",
        "Add an event to \(.applicationName)",
        "Schedule an event in \(.applicationName)",
        "New appointment in \(.applicationName)",
      ],
      shortTitle: "New event",
      systemImageName: "calendar.badge.plus"
    )
    AppShortcut(
      intent: CreateTaskIntent(),
      phrases: [
        "New task in \(.applicationName)",
        "Create a task in \(.applicationName)",
        "Create a new task in \(.applicationName)",
        "Add a task to \(.applicationName)",
        "New to-do in \(.applicationName)",
      ],
      shortTitle: "New task",
      systemImageName: "checklist"
    )
  }
}
