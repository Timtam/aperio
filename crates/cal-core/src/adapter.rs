//! Adapter traits.
//!
//! Every adapter implements the base [`Adapter`] trait plus at least one
//! of the three feature traits ([`CalendarFeature`], [`TasksFeature`],
//! [`ContactsFeature`]) matching its declared [`Capability`] list.
//!
//! See `DESIGN.md` section 6.1.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::color::ContainerColor;
use crate::error::{Error, Result};
use crate::reminder::SoundConfig;
use crate::types::{
    Calendar, Contact, DateRange, Event, FreeBusy, NewEvent, NewTask, Task, TaskList,
};

/// Stable identifier for the adapter source (e.g. "google", "caldav",
/// "vikunja").
///
/// Tagged onto every data object so the frontend can filter and show
/// source badges.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AdapterSource(pub String);

impl AdapterSource {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Adapter capabilities. An adapter declares which feature traits it
/// implements.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Capability {
    Calendar,
    Tasks,
    Contacts,
}

/// Credentials supplied when connecting. The concrete fields depend on the
/// auth scheme; each adapter documents the keys it expects.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Credentials {
    /// Key/value fields, e.g. `{"username": ..., "password": ...}` or
    /// `{"client_id": ..., "client_secret": ..., "refresh_token": ...}`.
    pub fields: std::collections::BTreeMap<String, String>,
}

/// Adapter-managed token. The adapter stores it internally (see note in
/// section 6.1) and does not require the caller to pass it explicitly.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AuthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Base trait for all adapters (section 6.1).
#[async_trait]
pub trait Adapter: Send + Sync {
    async fn authenticate(&self, credentials: Credentials) -> Result<AuthToken>;
    fn capabilities(&self) -> &[Capability];
}

/// Implemented by adapters that declare `Capability::Calendar`.
#[async_trait]
pub trait CalendarFeature: Adapter {
    async fn list_calendars(&self) -> Result<Vec<Calendar>>;
    async fn get_events(&self, calendar_id: &str, range: DateRange) -> Result<Vec<Event>>;
    async fn create_event(&self, calendar_id: &str, event: NewEvent) -> Result<Event>;
    async fn update_event(&self, event: Event) -> Result<Event>;
    async fn delete_event(&self, event_id: &str) -> Result<()>;
    async fn get_free_busy(&self, emails: &[&str], range: DateRange) -> Result<Vec<FreeBusy>>;
    fn calendar_color(&self, calendar_id: &str) -> Option<ContainerColor>;

    /// Append `occurrence` to the recurring event's EXDATE list so
    /// the expansion engine (and the source server) skips just that
    /// one date. The master row stays alive and every other
    /// occurrence keeps appearing — used by Aperio's "delete only
    /// this occurrence" flow on a series. Default implementation
    /// returns `Unsupported`; adapters that own the event data
    /// (local SQLite, CalDAV, …) override it.
    async fn add_event_exdate(
        &self,
        _event_id: &str,
        _occurrence: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "add_event_exdate is not supported on this adapter".into(),
        ))
    }
}

/// Implemented by adapters that declare `Capability::Tasks`.
#[async_trait]
pub trait TasksFeature: Adapter {
    async fn list_task_lists(&self) -> Result<Vec<TaskList>>;
    async fn get_tasks(&self, list_id: &str) -> Result<Vec<Task>>;
    async fn create_task(&self, list_id: &str, task: NewTask) -> Result<Task>;
    async fn update_task(&self, task: Task) -> Result<Task>;
    async fn delete_task(&self, task_id: &str) -> Result<()>;
}

/// Implemented by adapters that declare `Capability::Contacts`.
#[async_trait]
pub trait ContactsFeature: Adapter {
    async fn list_contacts(&self) -> Result<Vec<Contact>>;
    async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>>;
}

// ────────────────────────────────────────────────────────────────────────────
// Sound inheritance traits (section 14.4)
// ────────────────────────────────────────────────────────────────────────────

/// Implemented by items that may carry reminders (`Event`, `Task`).
pub trait Reminderable {
    /// Sound override at the item level; `None` ⇒ inherit from container.
    fn sound_override(&self) -> Option<&SoundConfig>;
}

/// Implemented by containers (`Calendar`, `TaskList`).
pub trait Container {
    /// Default sound for all items in this container; `None` ⇒ fall back
    /// to the app-wide global default.
    fn default_sound(&self) -> Option<&SoundConfig>;
}

impl Reminderable for crate::types::Event {
    fn sound_override(&self) -> Option<&SoundConfig> {
        self.sound.as_ref()
    }
}

impl Reminderable for crate::types::Task {
    fn sound_override(&self) -> Option<&SoundConfig> {
        self.sound.as_ref()
    }
}

impl Container for crate::types::Calendar {
    fn default_sound(&self) -> Option<&SoundConfig> {
        self.default_sound.as_ref()
    }
}

impl Container for crate::types::TaskList {
    fn default_sound(&self) -> Option<&SoundConfig> {
        self.default_sound.as_ref()
    }
}
