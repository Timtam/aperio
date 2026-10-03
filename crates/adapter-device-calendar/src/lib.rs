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

/// This adapter's manifest, embedded from the crate's own directory.
///
/// The adapter is linked in rather than loaded (see
/// `host_core::builtin_adapters`), but it is DESCRIBED like every other
/// adapter, and the host reads that description through this const rather than
/// reaching into this crate's directory. Bytes, not a parsed `PluginManifest`
/// (DESIGN.md section 20).
pub const MANIFEST: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/plugin.json"));

use std::sync::Arc;

use async_trait::async_trait;
use cal_core::os_access::{settled_by_grant, OsAccess};
use cal_core::{
    rrule_to_task_recurrence, Adapter, AdapterSource, AuthToken, Calendar, CalendarFeature,
    Capability, ContainerColor, Credentials, DateRange, Error, Event, FreeBusy, NewEvent, NewTask,
    Result, Task, TaskList, TaskPriority, TaskStatus, TasksFeature,
};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

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
    /// The OS's access state right now, without asking anyone: JSON
    /// `{"events": token, "reminders": token | null}` in the platform's own
    /// words, and on iOS `"granted_this_run": {"events": bool, "reminders":
    /// bool}`, the entities a request answered "granted" in this run.
    /// [`device_access`] reads it; the rule about what to do with it is
    /// `cal_core::os_access`, not the platform's.
    fn access_status(&self) -> String;

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

    /// Refuse unless the OS grants full access to `store` right now.
    ///
    /// Asked before every call, because without it the native store does not
    /// fail, it shows nothing: an empty catalog that read as "still loading",
    /// a delete of an event it cannot see that reported success, and under
    /// "add events only" one virtual calendar that would have replaced the
    /// real ones in the cache. Only full access passes (decision 171); every
    /// other state is the same refusal, naming the store and the state.
    fn require(&self, store: Store) -> Result<()> {
        let (calendar, tasks) = device_access(self.provider.as_ref());
        let (state, name) = match store {
            Store::Calendars => (calendar, "calendars"),
            Store::Reminders => (tasks.unwrap_or(OsAccess::Undetermined), "reminders"),
        };
        if state == OsAccess::Full {
            Ok(())
        } else {
            Err(Error::access_not_granted(format!("{name}: {state:?}")))
        }
    }
}

/// Which of the device's stores a call reads or writes.
#[derive(Clone, Copy)]
enum Store {
    Calendars,
    Reminders,
}

/// The platform's own words for an access state, in the core's.
///
/// iOS: EventKit's `EKAuthorizationStatus` (`full_access` also stands for the
/// pre-17 `authorized`). Android: the runtime permission plus Aperio's own
/// record of having asked on the device (decision 172) — `not_determined`
/// without the record is [`OsAccess::NotAsked`], `not_granted` after asking
/// is [`OsAccess::Undetermined`] (a refusal, or a dialog Android will not
/// show again). Anything else, a state a newer OS invents, is undetermined
/// too: never a reason to ask.
pub fn map_access_token(token: &str) -> OsAccess {
    match token {
        "full_access" | "granted" => OsAccess::Full,
        "write_only" => OsAccess::WriteOnly,
        "not_determined" => OsAccess::NotAsked,
        "denied" => OsAccess::Denied,
        "restricted" => OsAccess::Restricted,
        _ => OsAccess::Undetermined,
    }
}

#[derive(Deserialize)]
struct AccessStatusWire {
    events: String,
    #[serde(default)]
    reminders: Option<String>,
    /// iOS only: Android's status is read fresh from the permission itself
    /// and has no stale state to settle.
    #[serde(default)]
    granted_this_run: GrantedThisRun,
}

#[derive(Deserialize, Default)]
struct GrantedThisRun {
    #[serde(default)]
    events: bool,
    #[serde(default)]
    reminders: bool,
}

/// The device's access as the core goes by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceAccess {
    pub calendar: OsAccess,
    /// `None` where the platform has no reminders store.
    pub tasks: Option<OsAccess>,
    /// The OS stated "never asked" for the calendars although it granted them
    /// in this run, and the grant settled it (`os_access::settled_by_grant`).
    pub calendar_by_grant: bool,
    /// The same for the reminders.
    pub tasks_by_grant: bool,
}

impl DeviceAccess {
    /// The entities a grant settled, in the words a log line uses.
    pub fn settled_by_grant(&self) -> Vec<&'static str> {
        [
            (self.calendar_by_grant, "calendars"),
            (self.tasks_by_grant, "reminders"),
        ]
        .into_iter()
        .filter_map(|(settled, name)| settled.then_some(name))
        .collect()
    }
}

/// The calendars' and the reminders' access, as the core reads it. Reminders
/// are `None` where the platform has no store for them; a status the native
/// side could not put into words reads as undetermined.
pub fn device_access(provider: &dyn DeviceCalendarProvider) -> (OsAccess, Option<OsAccess>) {
    let DeviceAccess {
        calendar, tasks, ..
    } = read_device_access(provider);
    (calendar, tasks)
}

/// [`device_access`], with what a grant in this run settled.
///
/// One read of the native status for everything: two reads could straddle a
/// change and disagree.
pub fn read_device_access(provider: &dyn DeviceCalendarProvider) -> DeviceAccess {
    let raw = provider.access_status();
    let Ok(wire) = serde_json::from_str::<AccessStatusWire>(&raw) else {
        return DeviceAccess {
            calendar: OsAccess::Undetermined,
            tasks: provider
                .supports_reminders()
                .then_some(OsAccess::Undetermined),
            calendar_by_grant: false,
            tasks_by_grant: false,
        };
    };
    let stated_calendar = map_access_token(&wire.events);
    let calendar = settled_by_grant(stated_calendar, wire.granted_this_run.events);
    let stated_tasks = provider.supports_reminders().then(|| {
        wire.reminders
            .as_deref()
            .map_or(OsAccess::Undetermined, map_access_token)
    });
    let tasks =
        stated_tasks.map(|stated| settled_by_grant(stated, wire.granted_this_run.reminders));
    DeviceAccess {
        calendar,
        tasks,
        calendar_by_grant: calendar != stated_calendar,
        tasks_by_grant: tasks != stated_tasks,
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
        always_notifies_attendees: false,
        invitations_reply_only: false,
        stores_occurrence_exceptions: false,
        notifier_name: None,
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
        keep_attendees: false,
        keep_fields: Vec::new(),
        clear_attendees: false,
        organized_elsewhere: false,
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
        truncate_tail_overrides: false,
        created_at,
        updated_at,
        etag: None,
        organizer: None,
        attendee_responses: Vec::new(),
        // The native bridge doesn't surface EKEventStatus/Android STATUS yet,
        // and device-calendar events are OS-notified (Aperio never schedules
        // their reminders), so cancellation is moot here for now.
        cancelled: false,
        scheduling_silenced: false,
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
    /// The reminder's RFC-5545 RRULE body (no `RRULE:` prefix), when EventKit
    /// reports a recurrence rule. Parsed to a structured `TaskRecurrence` so
    /// Aperio can offer a scoped delete; `None` for a one-off reminder.
    #[serde(default)]
    recurrence: Option<String>,
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
        // Device calendars (EventKit / CalendarProvider) have no effort concept.
        effort: Default::default(),
        scheduled_date,
        scheduled_time,
        // EKReminder has a start and a due, and nothing that gives either a
        // length.
        scheduled_end_time: None,
        deadline_date: None,
        deadline_time: None,
        // Device calendars have no per-task deadline-countdown override.
        deadline_reminder_days: None,
        // EventKit's recurrence rule, mapped to the structured (lossy) model.
        // Lets Aperio recognise a repeating reminder and offer a scoped delete;
        // the OS still owns the recurrence lifecycle (see the device-account skip
        // in host update_task_json / the reminder enumerator).
        recurrence: d.recurrence.as_deref().and_then(rrule_to_task_recurrence),
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

// ── Write intermediate shapes (cal_core → native) ───────────────────────────
//
// The reverse of the read mapping: the adapter sends the native side only the
// EventKit-settable fields, the native side applies them and returns the
// resulting `DeviceEvent` / `DeviceReminder` (which maps back through the tested
// read path). Recurrence, colour, attendees, reminders and sections are out of
// P3 scope — they are not written (consistent with the reads dropping them).

#[derive(Debug, Clone, Serialize)]
struct DeviceEventWrite {
    /// Present on update (the event id to modify, possibly the occurrence-suffixed
    /// read id — the native side resolves it); absent on create.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    calendar_id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<String>,
    /// RFC-3339 instants.
    start: String,
    end: String,
    all_day: bool,
}

#[derive(Debug, Clone, Serialize)]
struct DeviceReminderWrite {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    list_id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    completed: bool,
    /// EventKit priority (0 unset / 1 high / 5 medium / 9 low).
    priority: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    due_time: Option<String>,
}

fn priority_to_ek(priority: TaskPriority) -> u8 {
    match priority {
        TaskPriority::High => 1,
        TaskPriority::Medium => 5,
        TaskPriority::Low => 9,
    }
}

fn event_write_create(calendar_id: &str, event: &NewEvent) -> DeviceEventWrite {
    DeviceEventWrite {
        id: None,
        calendar_id: calendar_id.to_string(),
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        start: event.start.to_rfc3339(),
        end: event.end.to_rfc3339(),
        all_day: event.all_day,
    }
}

fn event_write_update(event: &Event) -> DeviceEventWrite {
    DeviceEventWrite {
        id: Some(event.id.clone()),
        calendar_id: event.calendar_id.clone(),
        title: event.title.clone(),
        description: event.description.clone(),
        location: event.location.clone(),
        start: event.start.to_rfc3339(),
        end: event.end.to_rfc3339(),
        all_day: event.all_day,
    }
}

fn reminder_write_create(list_id: &str, task: &NewTask) -> DeviceReminderWrite {
    DeviceReminderWrite {
        id: None,
        list_id: list_id.to_string(),
        title: task.title.clone(),
        description: task.description.clone(),
        completed: task.status == TaskStatus::Completed,
        priority: priority_to_ek(task.priority),
        due_date: task
            .scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string()),
        due_time: task
            .scheduled_date
            .and(task.scheduled_time)
            .map(|t| t.format("%H:%M:%S").to_string()),
    }
}

fn reminder_write_update(task: &Task) -> DeviceReminderWrite {
    DeviceReminderWrite {
        id: Some(task.id.clone()),
        list_id: task.list_id.clone(),
        title: task.title.clone(),
        description: task.description.clone(),
        completed: task.status == TaskStatus::Completed,
        priority: priority_to_ek(task.priority),
        due_date: task
            .scheduled_date
            .map(|d| d.format("%Y-%m-%d").to_string()),
        due_time: task
            .scheduled_date
            .and(task.scheduled_time)
            .map(|t| t.format("%H:%M:%S").to_string()),
    }
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
        self.require(Store::Calendars)?;
        let devices: Vec<DeviceCalendar> = parse(&self.provider.list_calendars()?)?;
        Ok(devices.into_iter().map(map_calendar).collect())
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>> {
        self.require(Store::Calendars)?;
        let json = self.provider.get_events(
            calendar_id,
            &range.start.to_rfc3339(),
            &range.end.to_rfc3339(),
        )?;
        let devices: Vec<DeviceEvent> = parse(&json)?;
        devices.into_iter().map(map_event).collect()
    }

    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> Result<Event> {
        self.require(Store::Calendars)?;
        let write = event_write_create(calendar_id, &event);
        let json = self.provider.create_event(calendar_id, &to_json(&write)?)?;
        map_event(parse(&json)?)
    }

    async fn update_event(&self, event: Event) -> Result<Event> {
        self.require(Store::Calendars)?;
        let write = event_write_update(&event);
        let json = self.provider.update_event(&to_json(&write)?)?;
        map_event(parse(&json)?)
    }

    async fn delete_event(&self, event_id: &str, _send_cancellations: bool) -> Result<()> {
        self.require(Store::Calendars)?;
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
        self.require(Store::Reminders)?;
        let devices: Vec<DeviceReminderList> = parse(&self.provider.list_reminder_lists()?)?;
        Ok(devices.into_iter().map(map_reminder_list).collect())
    }

    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
        self.require(Store::Reminders)?;
        let devices: Vec<DeviceReminder> = parse(&self.provider.get_reminders(list_id)?)?;
        devices.into_iter().map(map_reminder).collect()
    }

    async fn create_task(&self, list_id: &str, task: NewTask) -> Result<Task> {
        self.require(Store::Reminders)?;
        let write = reminder_write_create(list_id, &task);
        let json = self.provider.create_reminder(list_id, &to_json(&write)?)?;
        map_reminder(parse(&json)?)
    }

    async fn update_task(&self, task: Task) -> Result<Task> {
        self.require(Store::Reminders)?;
        let write = reminder_write_update(&task);
        let json = self.provider.update_reminder(&to_json(&write)?)?;
        map_reminder(parse(&json)?)
    }

    async fn delete_task(&self, task_id: &str) -> Result<()> {
        self.require(Store::Reminders)?;
        self.provider.delete_reminder(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A provider that only answers the access question.
    struct AccessOnly {
        status: &'static str,
        reminders: bool,
    }

    impl DeviceCalendarProvider for AccessOnly {
        fn request_access(&self, _: bool, _: bool) -> Result<bool> {
            unreachable!()
        }
        fn supports_reminders(&self) -> bool {
            self.reminders
        }
        fn access_status(&self) -> String {
            self.status.to_string()
        }
        fn list_calendars(&self) -> Result<String> {
            unreachable!()
        }
        fn get_events(&self, _: &str, _: &str, _: &str) -> Result<String> {
            unreachable!()
        }
        fn create_event(&self, _: &str, _: &str) -> Result<String> {
            unreachable!()
        }
        fn update_event(&self, _: &str) -> Result<String> {
            unreachable!()
        }
        fn delete_event(&self, _: &str) -> Result<()> {
            unreachable!()
        }
        fn list_reminder_lists(&self) -> Result<String> {
            unreachable!()
        }
        fn get_reminders(&self, _: &str) -> Result<String> {
            unreachable!()
        }
        fn create_reminder(&self, _: &str, _: &str) -> Result<String> {
            unreachable!()
        }
        fn update_reminder(&self, _: &str) -> Result<String> {
            unreachable!()
        }
        fn delete_reminder(&self, _: &str) -> Result<()> {
            unreachable!()
        }
    }

    /// The device adapter's futures never wait: the provider is synchronous.
    fn ready<T>(future: impl std::future::Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(value) => value,
            std::task::Poll::Pending => panic!("the device adapter never waits"),
        }
    }

    fn refused<T: std::fmt::Debug>(result: Result<T>, what: &str) {
        match result {
            Err(Error::AccessNotGranted(_)) => {}
            other => panic!("{what}: expected AccessNotGranted, got {other:?}"),
        }
    }

    #[test]
    fn refuses_every_call_without_full_access() {
        // AccessOnly's data methods are unreachable: a call that got past the
        // gate would panic, so a refusal here is also "the store was never
        // touched" — no catalog read as empty, no delete reported as done.
        let range = DateRange::new(epoch(), epoch());
        for state in [
            "not_determined",
            "denied",
            "restricted",
            "write_only",
            "unknown_9",
        ] {
            let status: &'static str = Box::leak(
                format!(r#"{{"events":"{state}","reminders":"{state}"}}"#).into_boxed_str(),
            );
            let adapter = DeviceAdapter::new(Arc::new(AccessOnly {
                status,
                reminders: true,
            }));
            refused(ready(adapter.list_calendars()), state);
            refused(ready(adapter.get_events("cal", range)), state);
            refused(
                ready(adapter.create_event("cal", serde_json::from_str(NEW_EVENT).unwrap())),
                state,
            );
            refused(
                ready(adapter.update_event(serde_json::from_str(EVENT).unwrap())),
                state,
            );
            refused(ready(adapter.delete_event("ev", false)), state);
            refused(ready(adapter.list_task_lists()), state);
            refused(ready(adapter.get_tasks("list")), state);
            refused(
                ready(adapter.create_task("list", serde_json::from_str(NEW_TASK).unwrap())),
                state,
            );
            refused(
                ready(adapter.update_task(serde_json::from_str(TASK).unwrap())),
                state,
            );
            refused(ready(adapter.delete_task("task")), state);
        }
    }

    #[test]
    fn each_store_is_judged_on_its_own() {
        // Calendars granted, reminders not: the calendars pass the gate (and
        // reach the store, which here panics on purpose), the reminders do not.
        let adapter = DeviceAdapter::new(Arc::new(AccessOnly {
            status: r#"{"events":"full_access","reminders":"denied"}"#,
            reminders: true,
        }));
        refused(ready(adapter.list_task_lists()), "reminders denied");
        let reached = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = ready(adapter.list_calendars());
        }));
        assert!(reached.is_err(), "full access lets the calendars through");
    }

    #[test]
    fn a_grant_this_run_lets_a_store_stated_as_never_asked_through() {
        // The field case (decision 187): iOS answered "granted" and then
        // stated "never asked" for the reminders. The call reaches the store,
        // which here panics on purpose.
        let adapter = DeviceAdapter::new(Arc::new(AccessOnly {
            status: r#"{"events":"full_access","reminders":"not_determined","granted_this_run":{"events":true,"reminders":true}}"#,
            reminders: true,
        }));
        let reached = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = ready(adapter.list_task_lists());
        }));
        assert!(reached.is_err(), "the grant settles never asked");
        // Without the grant the same status is refused.
        let adapter = DeviceAdapter::new(Arc::new(AccessOnly {
            status: r#"{"events":"full_access","reminders":"not_determined"}"#,
            reminders: true,
        }));
        refused(ready(adapter.list_task_lists()), "reminders NotAsked");
    }

    #[test]
    fn reads_the_access_status_contract() {
        // The shapes both native bridges write; check-ffi-bridges.mjs holds
        // the writers' keys to the same samples. A key Rust does not read is
        // ignored without a word, so this is where a renamed one shows.
        const CONTRACT: &str = include_str!("../../../shared/contracts/deviceAccessStatus.json");
        let contract: serde_json::Value = serde_json::from_str(CONTRACT).unwrap();
        let samples = contract["samples"].as_array().unwrap();
        assert!(samples.len() >= 12, "the contract lost its samples");
        for sample in samples {
            let name = sample["name"].as_str().unwrap();
            let platform = sample["platform"].as_str();
            assert!(
                matches!(platform, Some("ios" | "android")),
                "{name}: platform {platform:?}"
            );
            let provider = AccessOnly {
                status: Box::leak(sample["status"].to_string().into_boxed_str()),
                reminders: platform == Some("ios"),
            };
            let calendar: OsAccess = serde_json::from_value(sample["calendar"].clone()).unwrap();
            let tasks: Option<OsAccess> = serde_json::from_value(sample["tasks"].clone()).unwrap();
            let settled: Vec<String> =
                serde_json::from_value(sample["settledByGrant"].clone()).unwrap();
            let read = read_device_access(&provider);
            assert_eq!(read.calendar, calendar, "{name}");
            assert_eq!(read.tasks, tasks, "{name}");
            assert_eq!(read.settled_by_grant(), settled, "{name}");
            assert_eq!(device_access(&provider), (calendar, tasks), "{name}");
        }
    }

    /// Minimal wire shapes for the write calls, never sent anywhere.
    const NEW_EVENT: &str = r#"{"title":"t","start":"2026-01-01T09:00:00Z","end":"2026-01-01T10:00:00Z","all_day":false,"reminders":[],"attendees":[]}"#;
    const EVENT: &str = r#"{"id":"ev","calendar_id":"cal","title":"t","start":"2026-01-01T09:00:00Z","end":"2026-01-01T10:00:00Z","all_day":false,"reminders":[],"attendees":[],"keep_fields":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#;
    const NEW_TASK: &str = r#"{"title":"t","status":"open","priority":"medium","reminders":[]}"#;
    const TASK: &str = r#"{"id":"task","list_id":"list","title":"t","status":"open","priority":"medium","reminders":[],"created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#;

    #[test]
    fn maps_access_tokens() {
        assert_eq!(map_access_token("full_access"), OsAccess::Full);
        assert_eq!(map_access_token("granted"), OsAccess::Full);
        assert_eq!(map_access_token("write_only"), OsAccess::WriteOnly);
        assert_eq!(map_access_token("not_determined"), OsAccess::NotAsked);
        assert_eq!(map_access_token("denied"), OsAccess::Denied);
        assert_eq!(map_access_token("restricted"), OsAccess::Restricted);
        // Android after asking: a refusal, or a dialog it will not show again.
        assert_eq!(map_access_token("not_granted"), OsAccess::Undetermined);
        // A state a newer OS invents is never a reason to ask.
        assert_eq!(map_access_token("unknown_9"), OsAccess::Undetermined);
    }

    #[test]
    fn reads_the_access_of_both_stores() {
        let ios = AccessOnly {
            status: r#"{"events":"not_determined","reminders":"full_access"}"#,
            reminders: true,
        };
        assert_eq!(
            device_access(&ios),
            (OsAccess::NotAsked, Some(OsAccess::Full))
        );
        // No reminders store: whatever the native side says about them is not
        // a state of anything.
        let android = AccessOnly {
            status: r#"{"events":"granted","reminders":null}"#,
            reminders: false,
        };
        assert_eq!(device_access(&android), (OsAccess::Full, None));
        // Android never asked on this device (no record of Aperio's): not
        // asked, so the start check asks once (decision 172). After asking:
        // undetermined, so "Allow access…" asks and finds out.
        let android_new = AccessOnly {
            status: r#"{"events":"not_determined","reminders":null}"#,
            reminders: false,
        };
        assert_eq!(device_access(&android_new), (OsAccess::NotAsked, None));
        let android_asked = AccessOnly {
            status: r#"{"events":"not_granted","reminders":null}"#,
            reminders: false,
        };
        assert_eq!(
            device_access(&android_asked),
            (OsAccess::Undetermined, None)
        );
        // Unreadable: undetermined, never "not asked".
        let garbled = AccessOnly {
            status: "?",
            reminders: true,
        };
        assert_eq!(
            device_access(&garbled),
            (OsAccess::Undetermined, Some(OsAccess::Undetermined))
        );
    }

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
            recurrence: None,
        }
    }

    #[test]
    fn maps_recurring_reminder_recurrence() {
        let mut r = reminder(false);
        r.due_date = Some("2026-06-25".into());
        r.recurrence = Some("FREQ=WEEKLY;BYDAY=MO,WE".into());
        let task = map_reminder(r).unwrap();
        let rec = task
            .recurrence
            .expect("recurrence populated from the rrule");
        assert_eq!(rec.frequency, cal_core::RecurrenceFrequency::Weekly);
        // A garbage / FREQ-less rule degrades cleanly to no recurrence.
        let mut bad = reminder(false);
        bad.recurrence = Some("not-a-rule".into());
        assert!(map_reminder(bad).unwrap().recurrence.is_none());
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

    fn new_event() -> NewEvent {
        NewEvent {
            organized_elsewhere: false,
            organizer: None,
            title: "Meeting".into(),
            description: Some("desc".into()),
            location: Some("HQ".into()),
            start: parse_instant("2026-06-21T09:00:00Z").unwrap(),
            end: parse_instant("2026-06-21T10:00:00Z").unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        }
    }

    fn new_task(status: TaskStatus, priority: TaskPriority) -> NewTask {
        NewTask {
            title: "Task".into(),
            description: None,
            status,
            priority,
            effort: Default::default(),
            deadline_reminder_days: None,
            scheduled_date: None,
            scheduled_time: None,
            scheduled_end_time: None,
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
        }
    }

    #[test]
    fn event_write_create_has_no_id_and_maps_fields() {
        let w = event_write_create("cal-1", &new_event());
        assert!(w.id.is_none());
        assert_eq!(w.calendar_id, "cal-1");
        assert_eq!(w.title, "Meeting");
        assert_eq!(w.description.as_deref(), Some("desc"));
        assert_eq!(w.start, "2026-06-21T09:00:00+00:00");
        assert!(!w.all_day);
    }

    #[test]
    fn event_write_update_carries_id() {
        let event = map_event(DeviceEvent {
            id: "ev#123".into(),
            calendar_id: "cal-1".into(),
            title: "X".into(),
            description: None,
            location: None,
            start: "2026-06-21T09:00:00Z".into(),
            end: "2026-06-21T10:00:00Z".into(),
            all_day: false,
            created_at: None,
            updated_at: None,
        })
        .unwrap();
        let w = event_write_update(&event);
        assert_eq!(w.id.as_deref(), Some("ev#123"));
        assert_eq!(w.calendar_id, "cal-1");
    }

    #[test]
    fn reminder_write_maps_priority_status_and_due() {
        let mut task = new_task(TaskStatus::Completed, TaskPriority::High);
        task.scheduled_date = NaiveDate::from_ymd_opt(2026, 6, 25);
        task.scheduled_time = NaiveTime::from_hms_opt(14, 30, 0);
        let w = reminder_write_create("l1", &task);
        assert!(w.id.is_none());
        assert!(w.completed);
        assert_eq!(w.priority, 1);
        assert_eq!(w.due_date.as_deref(), Some("2026-06-25"));
        assert_eq!(w.due_time.as_deref(), Some("14:30:00"));
    }

    #[test]
    fn reminder_write_drops_time_without_date() {
        let mut task = new_task(TaskStatus::Open, TaskPriority::Low);
        task.scheduled_time = NaiveTime::from_hms_opt(9, 0, 0);
        let w = reminder_write_create("l1", &task);
        assert_eq!(w.priority, 9);
        assert!(!w.completed);
        assert!(w.due_date.is_none());
        assert!(w.due_time.is_none());
    }

    #[test]
    fn priority_maps_to_ek_buckets() {
        assert_eq!(priority_to_ek(TaskPriority::High), 1);
        assert_eq!(priority_to_ek(TaskPriority::Medium), 5);
        assert_eq!(priority_to_ek(TaskPriority::Low), 9);
    }
}
