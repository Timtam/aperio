//! Device-local calendar + reminders adapter.
//!
//! Unlike the network adapters, this one owns no protocol code: it reads and
//! writes the **device's own** calendar and reminder stores — iOS EventKit
//! (`EKEvent` / `EKReminder`) and, later, Android `CalendarProvider`. Because
//! those native APIs are only reachable from Swift/Kotlin, the adapter holds a
//! [`DeviceCalendarProvider`] — a small, synchronous, JSON-in/JSON-out seam the
//! mobile layer (`cal-ffi`) backs with a UniFFI foreign trait whose Swift/Kotlin
//! implementations call the OS. The adapter itself is platform-agnostic Rust: it
//! maps the `cal_core` trait surface onto the provider and hands the parsed
//! domain objects back to the host.
//!
//! It is therefore **mobile-only** by construction — there is no desktop EventKit
//! — and is wired up in the `cal-ffi` host (which injects the native provider),
//! never loaded through the plugin manager. Its account is **device-local**: the
//! host never writes its `account.*` rows to the sync log, so it stays on the one
//! device that created it.
//!
//! See `DESIGN.md` §6 ("Lokale Kalender") and the mobile device-calendar plan.

use std::sync::Arc;

use async_trait::async_trait;
use cal_core::{
    Adapter, AdapterSource, AuthToken, Calendar, CalendarFeature, Capability, ContainerColor,
    Credentials, DateRange, Error, Event, FreeBusy, NewEvent, NewTask, Result, Task, TaskList,
    TaskPriority, TaskStatus, TasksFeature,
};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::Deserialize;

/// `AdapterSource` tag for every row this adapter owns.
pub const SOURCE_ID: &str = "device";

/// Synchronous bridge to the native device calendar/reminder store.
///
/// One method per operation the adapter needs. Containers and items cross the
/// boundary as JSON strings in the `cal_core` wire shape (the cal-ffi idiom):
/// the native side maps `EKEvent`/`EKReminder` → `Event`/`Task` JSON, and this
/// adapter only parses. Errors surface as [`cal_core::Error`] so the host treats
/// a device failure exactly like any other adapter's.
///
/// The boundary is **synchronous** on purpose: it mirrors `cal-ffi`'s
/// `KeychainBridge`, and the native side handles any internal async (EventKit
/// completion handlers) before returning. The host already runs the async
/// adapter methods on a worker via `block_on`, and device reads ride the SWR
/// cache rather than the render path, so a blocking native call is fine.
pub trait DeviceCalendarProvider: Send + Sync {
    /// Request OS permission for the selected entity types. Returns `true` iff
    /// access was granted. Drives the add-account "grant access" step.
    fn request_access(&self, events: bool, reminders: bool) -> Result<bool>;
    /// Whether this platform exposes a reminders/tasks store (iOS yes, Android
    /// no). Gates the [`Capability::Tasks`] declaration.
    fn supports_reminders(&self) -> bool;

    /// JSON `Vec<Calendar>`.
    fn list_calendars(&self) -> Result<String>;
    /// JSON `Vec<Event>` for `calendar_id` within `[start, end]` (RFC 3339).
    fn get_events(&self, calendar_id: &str, start: &str, end: &str) -> Result<String>;
    /// `event_json` is a `NewEvent`; returns the created `Event` as JSON.
    fn create_event(&self, calendar_id: &str, event_json: &str) -> Result<String>;
    /// `event_json` is an `Event`; returns the updated `Event` as JSON.
    fn update_event(&self, event_json: &str) -> Result<String>;
    fn delete_event(&self, event_id: &str) -> Result<()>;

    /// JSON `Vec<TaskList>` (the device's reminder lists).
    fn list_reminder_lists(&self) -> Result<String>;
    /// JSON `Vec<Task>` for one reminder list.
    fn get_reminders(&self, list_id: &str) -> Result<String>;
    /// `task_json` is a `NewTask`; returns the created `Task` as JSON.
    fn create_reminder(&self, list_id: &str, task_json: &str) -> Result<String>;
    /// `task_json` is a `Task`; returns the updated `Task` as JSON.
    fn update_reminder(&self, task_json: &str) -> Result<String>;
    fn delete_reminder(&self, task_id: &str) -> Result<()>;
}

/// The device-local calendar + reminders adapter.
pub struct DeviceAdapter {
    provider: Arc<dyn DeviceCalendarProvider>,
    source: AdapterSource,
    capabilities: Vec<Capability>,
}

impl DeviceAdapter {
    /// Build an adapter over a native provider. Declares `Tasks` only when the
    /// provider reports a reminders store (iOS), so the host gates the task UI
    /// off on Android.
    pub fn new(provider: Arc<dyn DeviceCalendarProvider>) -> Self {
        let mut capabilities = vec![Capability::Calendar];
        if provider.supports_reminders() {
            capabilities.push(Capability::Tasks);
        }
        Self {
            provider,
            source: AdapterSource::new(SOURCE_ID),
            capabilities,
        }
    }

    pub fn source(&self) -> &AdapterSource {
        &self.source
    }

    /// Run the native permission prompt for the selected entity types.
    pub fn request_access(&self, events: bool, reminders: bool) -> Result<bool> {
        self.provider.request_access(events, reminders)
    }
}

fn parse<T: serde::de::DeserializeOwned>(json: &str) -> Result<T> {
    serde_json::from_str(json).map_err(|e| Error::internal(format!("device adapter json: {e}")))
}

fn to_json<T: serde::Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::internal(format!("device adapter json: {e}")))
}

// ── Native intermediate shapes ──────────────────────────────────────────────
//
// The native side (Swift EventKit / Kotlin CalendarProvider) emits these
// SMALL, EventKit-shaped objects rather than the full `cal_core` `Calendar` /
// `Event` (16+ fields, nested colour/recurrence/reminder types, RFC-3339
// instants). The shape-correctness then lives in this (unit-tested) Rust
// mapping, not the un-typed native bridge — only the handful of fields EventKit
// natively provides cross the boundary; the adapter fills the rest with the
// "local-/read-only-source" defaults.

/// One device calendar as the native side reports it.
#[derive(Debug, Clone, Deserialize)]
struct DeviceCalendar {
    id: String,
    name: String,
    #[serde(default)]
    read_only: bool,
    /// `#RRGGBB`, when the platform exposes a per-calendar colour.
    #[serde(default)]
    color_hex: Option<String>,
}

/// One device event (an already-expanded occurrence within the queried window —
/// EventKit's predicate fetch returns concrete occurrences, so the adapter
/// treats each as standalone, `recurrence: None`).
#[derive(Debug, Clone, Deserialize)]
struct DeviceEvent {
    id: String,
    calendar_id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    location: Option<String>,
    /// RFC-3339 instants.
    start: String,
    end: String,
    #[serde(default)]
    all_day: bool,
    /// EventKit `creationDate` / `lastModifiedDate` (RFC-3339), when present —
    /// otherwise the adapter falls back to `start`.
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn parse_instant(s: &str) -> Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| Error::internal(format!("device adapter: bad instant {s:?}: {e}")))
}

fn map_calendar(d: DeviceCalendar) -> Calendar {
    Calendar {
        id: d.id,
        name: d.name,
        // The colour rides the row natively (the device store owns it); it never
        // round-trips to a remote, so no host-local override is needed.
        color: d.color_hex.map(ContainerColor::native),
        color_label: None,
        read_only: d.read_only,
        default_sound: None,
        supports_scheduling: false,
        supports_event_color: false,
    }
}

fn map_event(d: DeviceEvent) -> Result<Event> {
    let start = parse_instant(&d.start)?;
    let end = parse_instant(&d.end)?;
    let created_at = match &d.created_at {
        Some(s) => parse_instant(s)?,
        None => start,
    };
    let updated_at = match &d.updated_at {
        Some(s) => parse_instant(s)?,
        None => created_at,
    };
    Ok(Event {
        id: d.id,
        calendar_id: d.calendar_id,
        title: d.title,
        description: d.description,
        location: d.location,
        start,
        end,
        all_day: d.all_day,
        recurrence: None,
        color_label: None,
        color_hex: None,
        reminders: Vec::new(),
        sound: None,
        attendees: Vec::new(),
        send_invitations: false,
        created_at,
        updated_at,
        etag: None,
        organizer: None,
        attendee_responses: Vec::new(),
    })
}

/// One device reminder list (iOS Reminders list = `EKCalendar` of reminder
/// type) as the native side reports it.
#[derive(Debug, Clone, Deserialize)]
struct DeviceReminderList {
    id: String,
    name: String,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    color_hex: Option<String>,
}

/// One device reminder (`EKReminder`). Its due date maps to Aperio's
/// `scheduled_date` (iOS reminders are scheduled tasks, not deadlines).
#[derive(Debug, Clone, Deserialize)]
struct DeviceReminder {
    id: String,
    list_id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    completed: bool,
    /// EventKit `EKReminder.priority`: 0 = unset, 1-4 high, 5 medium, 6-9 low
    /// (RFC 5545).
    #[serde(default)]
    priority: u8,
    /// Due date `YYYY-MM-DD` (+ optional `HH:MM:SS` when the reminder has a time).
    #[serde(default)]
    due_date: Option<String>,
    #[serde(default)]
    due_time: Option<String>,
    /// RFC-3339 instants.
    #[serde(default)]
    completed_at: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

fn parse_date(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .map_err(|e| Error::internal(format!("device adapter: bad date {s:?}: {e}")))
}

fn parse_time(s: &str) -> Result<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S")
        .map_err(|e| Error::internal(format!("device adapter: bad time {s:?}: {e}")))
}

/// The deterministic last-resort timestamp when the native side reports none —
/// the Unix epoch. In practice the bridge always sends `created_at`
/// (`creationDate ?? Date()`), so this is purely defensive.
fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(0, 0).expect("unix epoch is valid")
}

fn map_priority(raw: u8) -> TaskPriority {
    match raw {
        1..=4 => TaskPriority::High,
        6..=9 => TaskPriority::Low,
        // 5 = medium; 0 = unset, treated as the neutral medium.
        _ => TaskPriority::Medium,
    }
}

fn map_reminder_list(d: DeviceReminderList) -> TaskList {
    TaskList {
        id: d.id,
        name: d.name,
        color: d.color_hex.map(ContainerColor::native),
        color_label: None,
        default_sound: None,
        embedded_in_calendar: None,
        parent_id: None,
        read_only: d.read_only,
    }
}

fn map_reminder(d: DeviceReminder) -> Result<Task> {
    let status = if d.completed {
        TaskStatus::Completed
    } else {
        TaskStatus::Open
    };
    let scheduled_date = d.due_date.as_deref().map(parse_date).transpose()?;
    // A time without a date is meaningless (and the DB CHECK forbids it) — drop it.
    let scheduled_time = match scheduled_date {
        Some(_) => d.due_time.as_deref().map(parse_time).transpose()?,
        None => None,
    };
    let completed_at = d.completed_at.as_deref().map(parse_instant).transpose()?;
    let created_at = match &d.created_at {
        Some(s) => parse_instant(s)?,
        None => completed_at.unwrap_or_else(epoch),
    };
    let updated_at = match &d.updated_at {
        Some(s) => parse_instant(s)?,
        None => created_at,
    };
    Ok(Task {
        id: d.id,
        list_id: d.list_id,
        title: d.title,
        description: d.description,
        status,
        priority: map_priority(d.priority),
        scheduled_date,
        scheduled_time,
        deadline_date: None,
        deadline_time: None,
        recurrence: None,
        resurface_date: None,
        series_id: None,
        parent_id: None,
        section_id: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        assignees: Vec::new(),
        created_at,
        updated_at,
        completed_at,
        etag: None,
    })
}

#[async_trait]
impl Adapter for DeviceAdapter {
    async fn authenticate(&self, _credentials: Credentials) -> Result<AuthToken> {
        // No remote auth — access is granted by the OS permission prompt at
        // add-account time, not a stored token.
        Ok(AuthToken::default())
    }

    fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }
}

#[async_trait]
impl CalendarFeature for DeviceAdapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>> {
        let devices: Vec<DeviceCalendar> = parse(&self.provider.list_calendars()?)?;
        Ok(devices.into_iter().map(map_calendar).collect())
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>> {
        let json = self.provider.get_events(
            calendar_id,
            &range.start.to_rfc3339(),
            &range.end.to_rfc3339(),
        )?;
        let devices: Vec<DeviceEvent> = parse(&json)?;
        devices.into_iter().map(map_event).collect()
    }

    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> Result<Event> {
        let json = self.provider.create_event(calendar_id, &to_json(&event)?)?;
        parse(&json)
    }

    async fn update_event(&self, event: Event) -> Result<Event> {
        let json = self.provider.update_event(&to_json(&event)?)?;
        parse(&json)
    }

    async fn delete_event(&self, event_id: &str, _send_cancellations: bool) -> Result<()> {
        // No server-side scheduling on a device calendar — the flag is ignored.
        self.provider.delete_event(event_id)
    }

    async fn get_free_busy(&self, _emails: &[&str], _range: DateRange) -> Result<Vec<FreeBusy>> {
        // The device store has no free/busy lookup; an empty result reads as
        // "no information", which is the correct degradation.
        Ok(vec![])
    }

    fn calendar_color(&self, _calendar_id: &str) -> Option<ContainerColor> {
        // Per-calendar colour rides on the `Calendar` rows from `list_calendars`;
        // there is no separate synchronous lookup on the device store.
        None
    }
}

#[async_trait]
impl TasksFeature for DeviceAdapter {
    async fn list_task_lists(&self) -> Result<Vec<TaskList>> {
        let devices: Vec<DeviceReminderList> = parse(&self.provider.list_reminder_lists()?)?;
        Ok(devices.into_iter().map(map_reminder_list).collect())
    }

    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
        let devices: Vec<DeviceReminder> = parse(&self.provider.get_reminders(list_id)?)?;
        devices.into_iter().map(map_reminder).collect()
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> Result<Task> {
        let json = self.provider.create_reminder(list_id, &to_json(&task)?)?;
        parse(&json)
    }

    async fn update_task(&self, task: Task) -> Result<Task> {
        let json = self.provider.update_reminder(&to_json(&task)?)?;
        parse(&json)
    }

    async fn delete_task(&self, task_id: &str) -> Result<()> {
        self.provider.delete_reminder(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_calendar_with_colour() {
        let c = map_calendar(DeviceCalendar {
            id: "cal-1".into(),
            name: "Work".into(),
            read_only: true,
            color_hex: Some("#4285f4".into()),
        });
        assert_eq!(c.id, "cal-1");
        assert_eq!(c.name, "Work");
        assert!(c.read_only);
        let color = c.color.expect("colour mapped");
        assert_eq!(color.hex, "#4285f4");
        assert!(c.color_label.is_none());
        assert!(!c.supports_event_color);
    }

    #[test]
    fn maps_calendar_without_colour() {
        let c = map_calendar(DeviceCalendar {
            id: "cal-2".into(),
            name: "Personal".into(),
            read_only: false,
            color_hex: None,
        });
        assert!(c.color.is_none());
        assert!(!c.read_only);
    }

    #[test]
    fn maps_event_and_falls_back_timestamps_to_start() {
        let e = map_event(DeviceEvent {
            id: "ev-1".into(),
            calendar_id: "cal-1".into(),
            title: "Standup".into(),
            description: None,
            location: Some("Room 4".into()),
            start: "2026-06-21T09:00:00Z".into(),
            end: "2026-06-21T09:30:00Z".into(),
            all_day: false,
            created_at: None,
            updated_at: None,
        })
        .expect("event maps");
        assert_eq!(e.id, "ev-1");
        assert_eq!(e.title, "Standup");
        assert_eq!(e.location.as_deref(), Some("Room 4"));
        assert_eq!(e.start.to_rfc3339(), "2026-06-21T09:00:00+00:00");
        // No created/updated provided ⇒ both fall back to start.
        assert_eq!(e.created_at, e.start);
        assert_eq!(e.updated_at, e.start);
        assert!(e.recurrence.is_none());
        assert!(e.reminders.is_empty());
    }

    #[test]
    fn maps_event_with_explicit_timestamps_and_normalises_to_utc() {
        let e = map_event(DeviceEvent {
            id: "ev-2".into(),
            calendar_id: "cal-1".into(),
            title: "Review".into(),
            description: Some("notes".into()),
            location: None,
            start: "2026-06-21T14:00:00+02:00".into(),
            end: "2026-06-21T15:00:00+02:00".into(),
            all_day: false,
            created_at: Some("2026-06-01T10:00:00Z".into()),
            updated_at: Some("2026-06-10T11:00:00Z".into()),
        })
        .expect("event maps");
        assert_eq!(e.start.to_rfc3339(), "2026-06-21T12:00:00+00:00");
        assert_eq!(e.created_at.to_rfc3339(), "2026-06-01T10:00:00+00:00");
        assert_eq!(e.updated_at.to_rfc3339(), "2026-06-10T11:00:00+00:00");
    }

    #[test]
    fn rejects_a_bad_instant() {
        let result = map_event(DeviceEvent {
            id: "ev-3".into(),
            calendar_id: "cal-1".into(),
            title: "Bad".into(),
            description: None,
            location: None,
            start: "not-a-date".into(),
            end: "2026-06-21T09:30:00Z".into(),
            all_day: false,
            created_at: None,
            updated_at: None,
        });
        assert!(result.is_err());
    }

    #[test]
    fn parses_native_calendar_json_omitting_default_fields() {
        // The exact shape the native side emits — omitting the serde-default
        // fields (read_only, color_hex) must still deserialise.
        let json = r##"[{"id":"c1","name":"Home"},
            {"id":"c2","name":"Work","read_only":true,"color_hex":"#ff0000"}]"##;
        let devices: Vec<DeviceCalendar> = parse(json).expect("parses");
        assert_eq!(devices.len(), 2);
        assert!(!devices[0].read_only);
        assert!(devices[0].color_hex.is_none());
        assert_eq!(devices[1].color_hex.as_deref(), Some("#ff0000"));
    }

    fn reminder(completed: bool) -> DeviceReminder {
        DeviceReminder {
            id: "r1".into(),
            list_id: "l1".into(),
            title: "Buy milk".into(),
            description: None,
            completed,
            priority: 0,
            due_date: None,
            due_time: None,
            completed_at: None,
            created_at: Some("2026-06-20T08:00:00Z".into()),
            updated_at: None,
        }
    }

    #[test]
    fn maps_open_reminder_with_due_date_and_time() {
        let mut r = reminder(false);
        r.due_date = Some("2026-06-25".into());
        r.due_time = Some("14:30:00".into());
        r.priority = 1;
        let task = map_reminder(r).expect("maps");
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.priority, TaskPriority::High);
        assert_eq!(task.scheduled_date.unwrap().to_string(), "2026-06-25");
        assert_eq!(task.scheduled_time.unwrap().to_string(), "14:30:00");
        assert!(task.deadline_date.is_none());
        assert!(task.completed_at.is_none());
        // updated_at falls back to created_at.
        assert_eq!(task.updated_at, task.created_at);
        assert_eq!(task.created_at.to_rfc3339(), "2026-06-20T08:00:00+00:00");
    }

    #[test]
    fn maps_completed_reminder() {
        let mut r = reminder(true);
        r.completed_at = Some("2026-06-21T10:00:00Z".into());
        let task = map_reminder(r).expect("maps");
        assert_eq!(task.status, TaskStatus::Completed);
        assert_eq!(
            task.completed_at.unwrap().to_rfc3339(),
            "2026-06-21T10:00:00+00:00"
        );
    }

    #[test]
    fn reminder_priority_buckets() {
        assert_eq!(map_priority(0), TaskPriority::Medium);
        assert_eq!(map_priority(3), TaskPriority::High);
        assert_eq!(map_priority(5), TaskPriority::Medium);
        assert_eq!(map_priority(7), TaskPriority::Low);
    }

    #[test]
    fn reminder_drops_time_without_date() {
        let mut r = reminder(false);
        r.due_date = None;
        r.due_time = Some("09:00:00".into());
        let task = map_reminder(r).expect("maps");
        assert!(task.scheduled_date.is_none());
        assert!(task.scheduled_time.is_none());
    }

    #[test]
    fn reminder_timestamp_falls_back_when_absent() {
        let mut r = reminder(false);
        r.created_at = None;
        r.completed_at = Some("2026-06-19T07:00:00Z".into());
        // No created_at ⇒ fall back to completed_at.
        let task = map_reminder(r).expect("maps");
        assert_eq!(task.created_at.to_rfc3339(), "2026-06-19T07:00:00+00:00");
    }
}
