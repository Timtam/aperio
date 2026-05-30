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
    Calendar, Contact, ContactList, ContactPhoto, DateRange, Event, FreeBusy, NewContact, NewEvent,
    NewTask, Section, Task, TaskList,
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

    /// Rename a calendar at the source. Local adapter does an SQL
    /// UPDATE on its `calendars.name`; CalDAV adapters issue
    /// PROPPATCH with `DAV:displayname` (RFC 4918 §15.2). iCal feed
    /// adapters and any other read-only source leave the default in
    /// place, which the command layer translates into "write a local
    /// override only".
    async fn rename_calendar(&self, _calendar_id: &str, _new_name: &str) -> Result<()> {
        Err(Error::Unsupported(
            "rename_calendar is not supported on this adapter".into(),
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

    /// Enumerate the sections (Vikunja buckets / Todoist sections) of a
    /// single list. Backends without the concept inherit the default
    /// empty list; adapters that model sections (Vikunja, Todoist)
    /// override it. Mirrors the default-`Ok(empty)` shape of the other
    /// optional feature methods.
    async fn list_sections(&self, _list_id: &str) -> Result<Vec<Section>> {
        Ok(vec![])
    }

    /// Rename a task list at the source — same semantics as
    /// `CalendarFeature::rename_calendar`: writable adapters override
    /// to push to the source, read-only adapters leave the default
    /// `Unsupported` and the command layer falls back to a local
    /// override.
    async fn rename_task_list(&self, _list_id: &str, _new_name: &str) -> Result<()> {
        Err(Error::Unsupported(
            "rename_task_list is not supported on this adapter".into(),
        ))
    }
}

/// Implemented by adapters that declare `Capability::Contacts`.
///
/// Symmetric with `CalendarFeature` / `TasksFeature`: enumerate the
/// address books (`list_contact_lists`), read the contacts in one
/// (`get_contacts`), and full CRUD on individual rows. Cross-list
/// `search_contacts` lives alongside the listing helpers so the
/// attendees-picker (§10.4) can hit it without first walking every
/// list.
///
/// `rename_contact_list` follows the same default-`Unsupported`
/// pattern as `CalendarFeature::rename_calendar` — read-only
/// providers (e.g. Google's "Other contacts") leave it alone and
/// the command layer falls back to a local override.
#[async_trait]
pub trait ContactsFeature: Adapter {
    async fn list_contact_lists(&self) -> Result<Vec<ContactList>>;
    async fn get_contacts(&self, list_id: &str) -> Result<Vec<Contact>>;
    /// Free-form text search across every list the adapter owns.
    /// The query is intended for the attendees-picker autocomplete;
    /// adapters MAY return an empty list if they can't service a
    /// quick search.
    async fn search_contacts(&self, query: &str) -> Result<Vec<Contact>>;
    async fn create_contact(&self, list_id: &str, contact: NewContact) -> Result<Contact>;
    async fn update_contact(&self, contact: Contact) -> Result<Contact>;
    async fn delete_contact(&self, contact_id: &str) -> Result<()>;

    async fn rename_contact_list(&self, _list_id: &str, _new_name: &str) -> Result<()> {
        Err(Error::Unsupported(
            "rename_contact_list is not supported on this adapter".into(),
        ))
    }

    /// Fetch the photo bytes for a contact. Pulled lazily because
    /// listings carry only the `has_photo` flag — a 1000-contact
    /// `get_contacts` shouldn't haul a megabyte of JPEGs across
    /// the IPC.
    ///
    /// `Ok(None)` ⇒ the contact exists but has no photo (the
    /// listing's `has_photo` would have been `false` anyway, but
    /// returning `None` rather than a `NotFound` keeps the caller
    /// from having to special-case the "stale flag" race).
    ///
    /// `Ok(Some(_))` ⇒ photo bytes plus their MIME content type.
    ///
    /// `Err(NotFound)` ⇒ the contact id itself doesn't exist.
    ///
    /// Adapters that don't model photos at all default to `Ok(None)`
    /// — the frontend renders the no-photo placeholder and the user
    /// sees no error.
    async fn get_contact_photo(&self, _contact_id: &str) -> Result<Option<ContactPhoto>> {
        Ok(None)
    }

    /// Replace the photo for an existing contact. Adapters that
    /// store photos inline (CardDAV vCard PHOTO) re-PUT the
    /// resource; adapters that store them as side-data (EWS
    /// ContactPicture attachment) issue the provider-specific
    /// attachment write. Default `Unsupported` so adapters that
    /// haven't grown the feature surface a clear error rather
    /// than silently swallowing the write.
    async fn set_contact_photo(&self, _contact_id: &str, _photo: ContactPhoto) -> Result<()> {
        Err(Error::Unsupported(
            "set_contact_photo is not supported on this adapter".into(),
        ))
    }

    /// Remove the photo without touching any other field. Same
    /// default-`Unsupported` shape as `set_contact_photo`.
    async fn delete_contact_photo(&self, _contact_id: &str) -> Result<()> {
        Err(Error::Unsupported(
            "delete_contact_photo is not supported on this adapter".into(),
        ))
    }

    /// Drop every in-memory cache the adapter holds for contact
    /// data — list-of-lists, per-list contact arrays, GAL pull
    /// snapshots. Surfaced as the back end of the
    /// "Einstellungen → Kontakte → Cache leeren" gesture in §10.6.
    ///
    /// Default no-op: the local adapter doesn't carry an in-memory
    /// cache (its data lives in SQLite, which is the source of
    /// truth — not a cache), so it inherits the no-op default
    /// without needing a bespoke impl. External adapters that
    /// hold listing / contact / photo caches override this to
    /// reset those structures; the next `get_contacts` then hits
    /// the wire and re-warms.
    ///
    /// Returning `Result` (rather than `()`) leaves room for an
    /// adapter that needs to flush an on-disk cache and could
    /// genuinely fail — say, an SQLite-backed CardDAV mirror that
    /// hits an IO error on truncate. The current external
    /// adapters all hold HashMaps in memory and can't fail.
    async fn invalidate_contacts_cache(&self) -> Result<()> {
        Ok(())
    }
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
