import CoreGraphics
import EventKit
import Foundation

/// iOS implementation of the Rust `DeviceEventStoreBridge` foreign trait — the
/// platform half of the device-local calendar + reminders adapter
/// (adapter-device-calendar). It reaches the device's own EventKit store
/// (`EKEvent` / `EKReminder`); the Rust adapter maps the small intermediate JSON
/// it exchanges here onto the full `cal_core` types, exactly as `IosKeychain`
/// backs the `SecretStore` seam.
///
/// `requestAccess` runs the real OS permission prompt; `supportsReminders` is
/// `true`. Reads (P1/P2) emit the intermediate calendar/event/reminder shape;
/// writes (P3) decode the intermediate write shape, apply it to EventKit, and
/// return the resulting item (which round-trips through the tested Rust read
/// mapping). Marked `@unchecked Sendable` because it holds a long-lived
/// `EKEventStore` (not `Sendable`); the store is internally thread-safe for the
/// single-call use here.
final class IosDeviceEventStore: DeviceEventStoreBridge, @unchecked Sendable {
  private let store = EKEventStore()

  /// The UniFFI boundary is synchronous, but EventKit's permission API is
  /// completion-based — block on a semaphore until it answers (the documented
  /// "native side owns the async" pattern). `events`/`reminders` select which
  /// entity types to request; both must be granted for the call to return true.
  func requestAccess(events: Bool, reminders: Bool) throws -> Bool {
    var granted = true
    if events {
      granted = requestEntity(.event) && granted
    }
    if reminders {
      granted = requestEntity(.reminder) && granted
    }
    return granted
  }

  private func requestEntity(_ type: EKEntityType) -> Bool {
    let semaphore = DispatchSemaphore(value: 0)
    var result = false
    let handler: EKEventStoreRequestAccessCompletionHandler = { ok, _ in
      result = ok
      semaphore.signal()
    }
    if #available(iOS 17.0, *) {
      switch type {
      case .event:
        store.requestFullAccessToEvents(completion: handler)
      case .reminder:
        store.requestFullAccessToReminders(completion: handler)
      @unknown default:
        store.requestAccess(to: type, completion: handler)
      }
    } else {
      store.requestAccess(to: type, completion: handler)
    }
    semaphore.wait()
    return result
  }

  func supportsReminders() -> Bool { true }

  /// Resolve a calendar/list identifier, giving the store ONE chance to
  /// finish loading its sources first. At cold launch
  /// `calendar(withIdentifier:)` can transiently return `nil` before the
  /// EKEventStore has loaded — treating that as "no such calendar → no
  /// items" poisoned the host's snapshot cache: the empty result replaced
  /// the warm rows and was stamped fresh, so the visible count collapsed
  /// until a later refresh repopulated it. `refreshSourcesIfNecessary()`
  /// nudges the load; a calendar that is STILL unresolvable afterwards is
  /// reported to the caller as an ERROR (the host then marks the container
  /// errored and keeps serving the cached rows) rather than as an empty
  /// collection. A genuinely removed calendar keeps erroring until the next
  /// listing refresh drops it from the catalog — after which nothing reads
  /// it anymore.
  private func resolveCalendar(_ identifier: String) -> EKCalendar? {
    if let calendar = store.calendar(withIdentifier: identifier) {
      return calendar
    }
    store.refreshSourcesIfNecessary()
    return store.calendar(withIdentifier: identifier)
  }

  /// Enumerate an entity type's calendars, nudging a not-yet-loaded store
  /// once (see resolveCalendar): a transiently EMPTY catalog at cold launch
  /// would replace the cached listing with nothing — the device calendars
  /// vanish from the sidebar (and their events out of the day views) until
  /// the next refresh. `refreshSourcesIfNecessary` is NOT documented as a
  /// synchronous load barrier, so an empty catalog after the nudge is still
  /// ambiguous — and a granted-access device virtually always has at least
  /// one calendar / reminder list (iOS maintains defaults). Treat empty as
  /// an ERROR: the host marks the listing errored and KEEPS the cached
  /// catalog, and the next refresh retries. On the rare device with truly
  /// zero lists the retry just keeps an already-empty cache empty (plus a
  /// log line per pass) — the safe side of the trade.
  private func loadedCalendars(for type: EKEntityType) throws -> [EKCalendar] {
    let calendars = store.calendars(for: type)
    if !calendars.isEmpty {
      return calendars
    }
    store.refreshSourcesIfNecessary()
    let retried = store.calendars(for: type)
    if retried.isEmpty {
      throw DeviceCalError.Backend(
        detail: "EventKit catalog empty (store may still be loading)")
    }
    return retried
  }

  // ── Calendar reads (P1) ──

  func listCalendars() throws -> String {
    let payload: [[String: Any]] = try loadedCalendars(for: .event).map { cal in
      var dict: [String: Any] = [
        "id": cal.calendarIdentifier,
        "name": cal.title,
        "read_only": !cal.allowsContentModifications,
      ]
      if let hex = Self.hexString(from: cal.cgColor) {
        dict["color_hex"] = hex
      }
      return dict
    }
    return try Self.encode(payload)
  }

  func getEvents(calendarId: String, start: String, end: String) throws -> String {
    guard let calendar = resolveCalendar(calendarId) else {
      // NOT "no events": an unresolvable identifier is an error, so the
      // host keeps its cached snapshot instead of replacing it with empty
      // (see resolveCalendar).
      throw DeviceCalError.Backend(
        detail: "calendar \(calendarId) not resolvable (store may still be loading)")
    }
    guard let startDate = Self.parseDate(start),
      let endDate = Self.parseDate(end)
    else {
      throw DeviceCalError.Backend(detail: "invalid event range: \(start)…\(end)")
    }
    let predicate = store.predicateForEvents(
      withStart: startDate, end: endDate, calendars: [calendar])
    // EventKit returns concrete (already-expanded) occurrences in the window.
    let payload = store.events(matching: predicate).map {
      Self.eventDict($0, calendarId: calendarId)
    }
    return try Self.encode(payload)
  }

  // ── Reminders reads (P2) ──

  func listReminderLists() throws -> String {
    let payload: [[String: Any]] = try loadedCalendars(for: .reminder).map { list in
      var dict: [String: Any] = [
        "id": list.calendarIdentifier,
        "name": list.title,
        "read_only": !list.allowsContentModifications,
      ]
      if let hex = Self.hexString(from: list.cgColor) {
        dict["color_hex"] = hex
      }
      return dict
    }
    return try Self.encode(payload)
  }

  func getReminders(listId: String) throws -> String {
    guard let list = resolveCalendar(listId) else {
      // See getEvents: an unresolvable identifier must not read as an
      // empty list — that would clobber the cached snapshot.
      throw DeviceCalError.Backend(
        detail: "reminder list \(listId) not resolvable (store may still be loading)")
    }
    // fetchReminders is completion-based — block on a semaphore across the sync
    // FFI boundary (as for the permission request).
    let predicate = store.predicateForReminders(in: [list])
    let semaphore = DispatchSemaphore(value: 0)
    var fetched: [EKReminder] = []
    store.fetchReminders(matching: predicate) { reminders in
      fetched = reminders ?? []
      semaphore.signal()
    }
    semaphore.wait()
    let payload = fetched.map { Self.reminderDict($0, listId: listId) }
    return try Self.encode(payload)
  }

  // ── Calendar writes (P3) ──

  func createEvent(calendarId: String, eventJson: String) throws -> String {
    let write = try Self.decode(EventWrite.self, eventJson)
    guard let calendar = store.calendar(withIdentifier: write.calendarId) else {
      throw DeviceCalError.Backend(detail: "unknown calendar \(write.calendarId)")
    }
    let event = EKEvent(eventStore: store)
    event.calendar = calendar
    try Self.apply(write, to: event)
    do {
      try store.save(event, span: .thisEvent, commit: true)
    } catch {
      throw DeviceCalError.Backend(detail: "save event: \(error.localizedDescription)")
    }
    return try Self.encode(Self.eventDict(event, calendarId: calendar.calendarIdentifier))
  }

  func updateEvent(eventJson: String) throws -> String {
    let write = try Self.decode(EventWrite.self, eventJson)
    guard let id = write.id,
      let event = store.event(withIdentifier: Self.baseEventId(id))
    else {
      throw DeviceCalError.Backend(detail: "event not found for update")
    }
    try Self.apply(write, to: event)
    do {
      try store.save(event, span: .thisEvent, commit: true)
    } catch {
      throw DeviceCalError.Backend(detail: "save event: \(error.localizedDescription)")
    }
    return try Self.encode(
      Self.eventDict(event, calendarId: event.calendar.calendarIdentifier))
  }

  func deleteEvent(eventId: String) throws {
    guard let event = store.event(withIdentifier: Self.baseEventId(eventId)) else {
      // Already gone — delete is idempotent.
      return
    }
    do {
      try store.remove(event, span: .thisEvent, commit: true)
    } catch {
      throw DeviceCalError.Backend(detail: "remove event: \(error.localizedDescription)")
    }
  }

  // ── Reminder writes (P3) ──

  func createReminder(listId: String, taskJson: String) throws -> String {
    let write = try Self.decode(ReminderWrite.self, taskJson)
    guard let list = store.calendar(withIdentifier: write.listId) else {
      throw DeviceCalError.Backend(detail: "unknown reminder list \(write.listId)")
    }
    let reminder = EKReminder(eventStore: store)
    reminder.calendar = list
    Self.apply(write, to: reminder)
    do {
      try store.save(reminder, commit: true)
    } catch {
      throw DeviceCalError.Backend(detail: "save reminder: \(error.localizedDescription)")
    }
    return try Self.encode(Self.reminderDict(reminder, listId: list.calendarIdentifier))
  }

  func updateReminder(taskJson: String) throws -> String {
    let write = try Self.decode(ReminderWrite.self, taskJson)
    guard let id = write.id,
      let reminder = store.calendarItem(withIdentifier: id) as? EKReminder
    else {
      throw DeviceCalError.Backend(detail: "reminder not found for update")
    }
    Self.apply(write, to: reminder)
    do {
      try store.save(reminder, commit: true)
    } catch {
      throw DeviceCalError.Backend(detail: "save reminder: \(error.localizedDescription)")
    }
    return try Self.encode(
      Self.reminderDict(reminder, listId: reminder.calendar.calendarIdentifier))
  }

  func deleteReminder(taskId: String) throws {
    guard let reminder = store.calendarItem(withIdentifier: taskId) as? EKReminder else {
      return  // Already gone — idempotent.
    }
    do {
      try store.remove(reminder, commit: true)
    } catch {
      throw DeviceCalError.Backend(detail: "remove reminder: \(error.localizedDescription)")
    }
  }

  // ── Write payloads (the small cal_core→native shape; snake_case) ──

  private struct EventWrite: Decodable {
    let id: String?
    let calendarId: String
    let title: String
    let description: String?
    let location: String?
    let start: String
    let end: String
    let allDay: Bool
  }

  private struct ReminderWrite: Decodable {
    let id: String?
    let listId: String
    let title: String
    let description: String?
    let completed: Bool
    let priority: Int
    let dueDate: String?
    let dueTime: String?
  }

  private static func apply(_ write: EventWrite, to event: EKEvent) throws {
    guard let start = iso.date(from: write.start), let end = iso.date(from: write.end) else {
      throw DeviceCalError.Backend(detail: "invalid event dates: \(write.start)…\(write.end)")
    }
    event.title = write.title
    event.notes = write.description
    event.location = write.location
    event.isAllDay = write.allDay
    event.startDate = start
    event.endDate = end
  }

  private static func apply(_ write: ReminderWrite, to reminder: EKReminder) {
    reminder.title = write.title
    reminder.notes = write.description
    reminder.isCompleted = write.completed
    reminder.priority = write.priority
    let due = dueComponents(date: write.dueDate, time: write.dueTime)
    reminder.dueDateComponents = due
    // Apple: "On iOS, Event Kit requires that a start date is set if the due
    // date is set." Aperio has been writing due-only and the saves went
    // through, so the requirement is not enforced at runtime — but a reminder
    // that contradicts its own store's documented shape is a bad thing to keep
    // producing, and the start is also what Apple's own grouping reads.
    //
    // The day, never a time: a start time would be a second clock Aperio does
    // not hold and the user never chose. And only when the reminder has NO
    // start yet — one set in Reminders or another client is that client's
    // statement about the task, and overwriting it with our due day would
    // silently move it.
    if let due = due, reminder.startDateComponents == nil {
      var start = DateComponents()
      start.year = due.year
      start.month = due.month
      start.day = due.day
      reminder.startDateComponents = start
    }
  }

  // ── Shared dict builders (read responses + write responses) ──

  private static func eventDict(_ event: EKEvent, calendarId: String) -> [String: Any] {
    var dict: [String: Any] = [
      "id": eventId(event),
      "calendar_id": calendarId,
      "title": event.title ?? "",
      "start": iso.string(from: event.startDate),
      "end": iso.string(from: event.endDate),
      "all_day": event.isAllDay,
    ]
    if let notes = event.notes { dict["description"] = notes }
    if let location = event.location { dict["location"] = location }
    if let created = event.creationDate { dict["created_at"] = iso.string(from: created) }
    if let modified = event.lastModifiedDate { dict["updated_at"] = iso.string(from: modified) }
    return dict
  }

  private static func reminderDict(_ reminder: EKReminder, listId: String) -> [String: Any] {
    let due = dateComponentsStrings(reminder.dueDateComponents)
    var dict: [String: Any] = [
      "id": reminder.calendarItemIdentifier,
      "list_id": listId,
      "title": reminder.title ?? "",
      "completed": reminder.isCompleted,
      "priority": reminder.priority,
    ]
    if let notes = reminder.notes { dict["description"] = notes }
    if let date = due.date { dict["due_date"] = date }
    if let time = due.time { dict["due_time"] = time }
    if let completion = reminder.completionDate {
      dict["completed_at"] = iso.string(from: completion)
    }
    dict["created_at"] = iso.string(from: reminder.creationDate ?? Date())
    dict["updated_at"] = iso.string(
      from: reminder.lastModifiedDate ?? reminder.creationDate ?? Date())
    // iOS Reminders carry at most one rule; expose it as an RRULE body so
    // cal_core can recognise the repeat and Aperio can offer a scoped delete.
    if let rule = reminder.recurrenceRules?.first {
      dict["recurrence"] = Self.rrule(from: rule)
    }
    return dict
  }

  /// Compact UTC formatter for an RRULE `UNTIL` (`yyyyMMddTHHmmssZ`). MUST NOT be
  /// the `iso` formatter: cal_core parses the date part with `%Y%m%d` after
  /// splitting on `T`, and the ISO-8601 dashed form (`2026-06-25`) fails that
  /// parse, silently dropping the end bound and making the series read endless.
  private static let rruleUntil: DateFormatter = {
    let f = DateFormatter()
    f.dateFormat = "yyyyMMdd'T'HHmmss'Z'"
    f.timeZone = TimeZone(identifier: "UTC")
    f.locale = Locale(identifier: "en_US_POSIX")
    return f
  }()

  private static func rruleDay(_ weekday: EKWeekday) -> String {
    switch weekday {
    case .sunday: return "SU"
    case .monday: return "MO"
    case .tuesday: return "TU"
    case .wednesday: return "WE"
    case .thursday: return "TH"
    case .friday: return "FR"
    case .saturday: return "SA"
    @unknown default: return "MO"
    }
  }

  /// Serialize an EventKit recurrence rule to an RFC-5545 RRULE body (no
  /// `RRULE:` prefix). Only the parts cal_core models are emitted — FREQ,
  /// INTERVAL, BYDAY, BYMONTHDAY, and COUNT or UNTIL; richer EventKit parts
  /// (e.g. relative "2nd Monday", BYMONTH, setpos) are dropped, matching the
  /// structured model's documented lossiness.
  static func rrule(from rule: EKRecurrenceRule) -> String {
    var parts: [String] = []
    switch rule.frequency {
    case .daily: parts.append("FREQ=DAILY")
    case .weekly: parts.append("FREQ=WEEKLY")
    case .monthly: parts.append("FREQ=MONTHLY")
    case .yearly: parts.append("FREQ=YEARLY")
    @unknown default: parts.append("FREQ=DAILY")
    }
    if rule.interval > 1 {
      parts.append("INTERVAL=\(rule.interval)")
    }
    if let days = rule.daysOfTheWeek, !days.isEmpty {
      let byday = days.map { Self.rruleDay($0.dayOfTheWeek) }.joined(separator: ",")
      parts.append("BYDAY=\(byday)")
    }
    if let monthDays = rule.daysOfTheMonth, !monthDays.isEmpty {
      let byMonthDay = monthDays.map { "\($0.intValue)" }.joined(separator: ",")
      parts.append("BYMONTHDAY=\(byMonthDay)")
    }
    if let end = rule.recurrenceEnd {
      if end.occurrenceCount > 0 {
        parts.append("COUNT=\(end.occurrenceCount)")
      } else if let endDate = end.endDate {
        parts.append("UNTIL=\(Self.rruleUntil.string(from: endDate))")
      }
    }
    return parts.joined(separator: ";")
  }

  // ── Encoding / decoding helpers ──

  private static let iso: ISO8601DateFormatter = {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime]
    return formatter
  }()

  /// RFC-3339 with fractional seconds. `iso` above rejects them (an
  /// ISO8601DateFormatter only parses the exact shape its options
  /// describe), but the Rust host derives range endpoints from
  /// `Utc::now()`-precision timestamps in places — the whole-second and
  /// fractional variants must BOTH parse or a refresh window silently
  /// becomes "invalid event range".
  private static let isoFractional: ISO8601DateFormatter = {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    return formatter
  }()

  /// Parse an RFC-3339 timestamp with or without fractional seconds.
  private static func parseDate(_ value: String) -> Date? {
    iso.date(from: value) ?? isoFractional.date(from: value)
  }

  /// EventKit reuses `eventIdentifier` across a recurring series' occurrences, so
  /// suffix the occurrence start to give each expanded instance a distinct
  /// cal_core `Event` id. [`baseEventId`] strips it back off for writes.
  private static func eventId(_ event: EKEvent) -> String {
    let base = event.eventIdentifier ?? event.calendarItemIdentifier
    return "\(base)#\(Int(event.startDate.timeIntervalSince1970))"
  }

  /// Strip the occurrence suffix `eventId` appends, recovering the EventKit
  /// identifier for `event(withIdentifier:)`.
  private static func baseEventId(_ id: String) -> String {
    if let hashIndex = id.lastIndex(of: "#") {
      return String(id[..<hashIndex])
    }
    return id
  }

  /// `YYYY-MM-DD` (+ optional `HH:MM:SS`) → due `DateComponents`, or nil for no
  /// due date.
  ///
  /// No `timeZone` is set, and that is a choice rather than an omission. Apple
  /// documents both "a nil time zone represents a floating date" and "by
  /// default, the due date is set to the system time zone", which cannot both
  /// be the whole story — but floating is what Aperio actually holds: a naive
  /// wall clock the user typed, with no zone attached. It is the same answer
  /// the CalDAV task writer gives for the same reason, and it is the one that
  /// keeps "09:00" reading as 09:00 wherever the phone happens to be.
  ///
  /// A reminder's `startDateComponents` is not read back into the model: with
  /// the due mapped to `scheduled_date` there is no slot left for it, and the
  /// write path deliberately leaves an existing one alone rather than
  /// overwriting what another client stored.
  private static func dueComponents(date: String?, time: String?) -> DateComponents? {
    guard let date = date else { return nil }
    let dateParts = date.split(separator: "-")
    guard dateParts.count == 3, let year = Int(dateParts[0]),
      let month = Int(dateParts[1]), let day = Int(dateParts[2])
    else {
      return nil
    }
    var components = DateComponents()
    components.year = year
    components.month = month
    components.day = day
    if let time = time {
      let timeParts = time.split(separator: ":")
      if timeParts.count >= 2, let hour = Int(timeParts[0]), let minute = Int(timeParts[1]) {
        components.hour = hour
        components.minute = minute
        if timeParts.count >= 3, let second = Int(timeParts[2]) {
          components.second = second
        }
      }
    }
    return components
  }

  private static func encode(_ object: Any) throws -> String {
    let data = try JSONSerialization.data(withJSONObject: object, options: [])
    guard let json = String(data: data, encoding: .utf8) else {
      throw DeviceCalError.Backend(detail: "could not encode device payload as UTF-8")
    }
    return json
  }

  private static func decode<T: Decodable>(_ type: T.Type, _ json: String) throws -> T {
    guard let data = json.data(using: .utf8) else {
      throw DeviceCalError.Backend(detail: "write payload was not valid UTF-8")
    }
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    do {
      return try decoder.decode(T.self, from: data)
    } catch {
      throw DeviceCalError.Backend(detail: "decode write payload: \(error.localizedDescription)")
    }
  }

  /// A reminder's `dueDateComponents` → (`YYYY-MM-DD`, optional `HH:MM:SS`).
  private static func dateComponentsStrings(
    _ components: DateComponents?
  ) -> (date: String?, time: String?) {
    guard let components = components, let year = components.year,
      let month = components.month, let day = components.day
    else {
      return (nil, nil)
    }
    let date = String(format: "%04d-%02d-%02d", year, month, day)
    if let hour = components.hour, let minute = components.minute {
      let second = components.second ?? 0
      return (date, String(format: "%02d:%02d:%02d", hour, minute, second))
    }
    return (date, nil)
  }

  /// `#RRGGBB` from an EKCalendar's CGColor (RGB color spaces only; grayscale /
  /// unknown spaces yield nil → the calendar renders without a colour).
  private static func hexString(from cgColor: CGColor?) -> String? {
    guard let components = cgColor?.components, components.count >= 3 else {
      return nil
    }
    let r = Int((components[0] * 255).rounded())
    let g = Int((components[1] * 255).rounded())
    let b = Int((components[2] * 255).rounded())
    return String(format: "#%02x%02x%02x", r, g, b)
  }
}
