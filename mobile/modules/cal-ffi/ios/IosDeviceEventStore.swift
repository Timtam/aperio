import CoreGraphics
import EventKit
import Foundation

/// iOS implementation of the Rust `DeviceEventStoreBridge` foreign trait — the
/// platform half of the device-local calendar + reminders adapter
/// (cal-adapter-device-calendar). It reaches the device's own EventKit store
/// (`EKEvent` / `EKReminder`); the Rust adapter maps the JSON it returns onto the
/// `cal_core` trait surface, exactly as `IosKeychain` backs the `SecretStore`
/// seam.
///
/// **P0 scaffolding:** `requestAccess` runs the real OS permission prompt (so the
/// add-account "grant access" step works end to end) and `supportsReminders` is
/// `true`. The data reads return empty and the writes throw "later phase" — P1
/// wires `listCalendars`/`getEvents` over `EKEvent`, P2 the reminders over
/// `EKReminder`, P3 the writes. Marked `@unchecked Sendable` because it holds a
/// long-lived `EKEventStore` (not `Sendable`); the store is internally
/// thread-safe for the single-call use here.
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

  // ── Calendar reads (P1) ──
  // Emit the device-adapter's small intermediate shape (id/name/read_only/
  // color_hex for calendars; id/calendar_id/title/start/end/… for events). The
  // Rust adapter maps these onto the full cal_core `Calendar`/`Event`, so the
  // shape-correctness lives in tested Rust, not here.

  func listCalendars() throws -> String {
    let payload: [[String: Any]] = store.calendars(for: .event).map { cal in
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
    guard let calendar = store.calendar(withIdentifier: calendarId) else {
      // Unknown / removed calendar — no events, not an error.
      return "[]"
    }
    guard let startDate = Self.iso.date(from: start),
      let endDate = Self.iso.date(from: end)
    else {
      throw DeviceCalError.Backend(detail: "invalid event range: \(start)…\(end)")
    }
    let predicate = store.predicateForEvents(
      withStart: startDate, end: endDate, calendars: [calendar])
    // EventKit returns concrete (already-expanded) occurrences in the window;
    // each is mapped to a standalone Event (recurrence: none) for P1 reads.
    let payload: [[String: Any]] = store.events(matching: predicate).map { event in
      var dict: [String: Any] = [
        "id": Self.eventId(event),
        "calendar_id": calendarId,
        "title": event.title ?? "",
        "start": Self.iso.string(from: event.startDate),
        "end": Self.iso.string(from: event.endDate),
        "all_day": event.isAllDay,
      ]
      if let notes = event.notes { dict["description"] = notes }
      if let location = event.location { dict["location"] = location }
      if let created = event.creationDate {
        dict["created_at"] = Self.iso.string(from: created)
      }
      if let modified = event.lastModifiedDate {
        dict["updated_at"] = Self.iso.string(from: modified)
      }
      return dict
    }
    return try Self.encode(payload)
  }

  // ── Reminders reads (P2) ──
  func listReminderLists() throws -> String { "[]" }
  func getReminders(listId: String) throws -> String { "[]" }

  // ── Encoding helpers ──

  private static let iso: ISO8601DateFormatter = {
    let formatter = ISO8601DateFormatter()
    formatter.formatOptions = [.withInternetDateTime]
    return formatter
  }()

  /// EventKit reuses `eventIdentifier` across a recurring series' occurrences, so
  /// suffix the occurrence start to give each expanded instance a distinct
  /// cal_core `Event` id.
  private static func eventId(_ event: EKEvent) -> String {
    let base = event.eventIdentifier ?? event.calendarItemIdentifier
    return "\(base)#\(Int(event.startDate.timeIntervalSince1970))"
  }

  private static func encode(_ payload: [[String: Any]]) throws -> String {
    let data = try JSONSerialization.data(withJSONObject: payload, options: [])
    guard let json = String(data: data, encoding: .utf8) else {
      throw DeviceCalError.Backend(detail: "could not encode device payload as UTF-8")
    }
    return json
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

  // ── Writes (P3) ──
  func createEvent(calendarId: String, eventJson: String) throws -> String {
    throw DeviceCalError.Backend(detail: "device calendar writes arrive in a later phase")
  }
  func updateEvent(eventJson: String) throws -> String {
    throw DeviceCalError.Backend(detail: "device calendar writes arrive in a later phase")
  }
  func deleteEvent(eventId: String) throws {
    throw DeviceCalError.Backend(detail: "device calendar writes arrive in a later phase")
  }
  func createReminder(listId: String, taskJson: String) throws -> String {
    throw DeviceCalError.Backend(detail: "device reminder writes arrive in a later phase")
  }
  func updateReminder(taskJson: String) throws -> String {
    throw DeviceCalError.Backend(detail: "device reminder writes arrive in a later phase")
  }
  func deleteReminder(taskId: String) throws {
    throw DeviceCalError.Backend(detail: "device reminder writes arrive in a later phase")
  }
}
