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

  // ── Reads (P1/P2 fill these from EventKit) ──
  func listCalendars() throws -> String { "[]" }
  func getEvents(calendarId: String, start: String, end: String) throws -> String { "[]" }
  func listReminderLists() throws -> String { "[]" }
  func getReminders(listId: String) throws -> String { "[]" }

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
