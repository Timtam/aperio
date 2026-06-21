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
    TasksFeature,
};

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
        parse(&self.provider.list_calendars()?)
    }

    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>> {
        let json =
            self.provider
                .get_events(calendar_id, &range.start.to_rfc3339(), &range.end.to_rfc3339())?;
        parse(&json)
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
        parse(&self.provider.list_reminder_lists()?)
    }

    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>> {
        parse(&self.provider.get_reminders(list_id)?)
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
