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
    AttendeeStatus, Calendar, Contact, ContactList, ContactPhoto, DateRange, Event, FreeBusy,
    MemberRight, NewContact, NewEvent, NewTask, Section, Task, TaskList, TaskListShare, TaskUser,
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
    /// Delete an event. When `send_cancellations` is `true` AND the event is
    /// a meeting the connected account organises, scheduling-capable
    /// providers email attendees a cancellation (EWS `SendToAllAndSaveCopy`,
    /// CalDAV RFC 6638, Google `sendUpdates=all`). Adapters without
    /// server-side scheduling ignore the flag.
    async fn delete_event(&self, event_id: &str, send_cancellations: bool) -> Result<()>;
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

    /// Incremental events fetch (CACHE-4). `since_token` is the opaque
    /// cursor returned by a prior call (`None` on the first/bootstrap
    /// call, which should return the full window as a `full_resync`).
    /// Default `Unsupported` — the host falls back to a full
    /// [`CalendarFeature::get_events`]. Delta-capable adapters
    /// (Google syncToken, Graph deltaLink, CalDAV sync-collection,
    /// EWS SyncFolderItems) override it.
    async fn get_events_delta(
        &self,
        _calendar_id: &str,
        _range: DateRange,
        _since_token: Option<&str>,
    ) -> Result<ChangeSet<Event>> {
        Err(Error::Unsupported(
            "get_events_delta is not supported on this adapter".into(),
        ))
    }

    /// The email address of the connected account's user, when the
    /// provider can report it. Lets the host decide whether the user is
    /// an *attendee* of a meeting (and not its organizer) — the gate for
    /// the RSVP affordance. Default `None` (identity unknown / not
    /// applicable, e.g. local + iCal feeds). Implemented per provider:
    /// CalDAV from the discovered `calendar-user-address`, Graph from
    /// `/me`, Google from the token's userinfo, EWS from the configured
    /// login. A failure to determine it degrades to `None`, never an
    /// error — "unknown identity" simply hides the RSVP buttons.
    async fn current_user_email(&self) -> Result<Option<String>> {
        Ok(None)
    }

    /// RSVP to an invitation: set the connected user's participation
    /// status on `event_id` to `status`. When `send_response` is `true`
    /// AND the provider schedules server-side, it also emails the reply
    /// to the organizer (EWS `AcceptItem`/`DeclineItem`, Graph
    /// `/accept|/decline`, Google `sendUpdates`, CalDAV iTIP `REPLY`).
    /// Default `Unsupported`; the four external calendar adapters
    /// override it. Only meaningful for an event the user is an attendee
    /// (not organizer) of — the caller gates on
    /// [`current_user_email`](Self::current_user_email).
    async fn respond_to_event(
        &self,
        _event_id: &str,
        _status: AttendeeStatus,
        _send_response: bool,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "respond_to_event is not supported on this adapter".into(),
        ))
    }
}

/// Result of an incremental ("delta") fetch.
///
/// Returned by the optional `get_*_delta` feature methods. The host
/// applies it to its snapshot cache: upsert `changes`, drop `deletions`,
/// then persist `new_token` for the next round. `full_resync` tells the
/// host the server invalidated the prior token (or this was a first
/// bootstrap with `since_token == None`) and `changes` is the COMPLETE
/// set for the queried scope — the host replaces wholesale rather than
/// merging.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeSet<T> {
    /// Created or updated rows since the token.
    pub changes: Vec<T>,
    /// Native ids removed since the token.
    #[serde(default)]
    pub deletions: Vec<String>,
    /// Opaque token to pass back next time (sync token / deltaLink /
    /// CTag-derived cursor). `None` ⇒ the adapter can't continue
    /// incrementally and the host should treat the next round as cold.
    #[serde(default)]
    pub new_token: Option<String>,
    /// `true` ⇒ `changes` is the full set; replace, don't merge.
    #[serde(default)]
    pub full_resync: bool,
    /// `true` ⇒ `changes` covers the adapter's ENTIRE collection
    /// (folder-complete), not merely the queried `range`. Folder-complete
    /// adapters (EWS `SyncFolderItems`, CalDAV `sync-collection`) keep the
    /// whole folder current via delta, so the host may record an unbounded
    /// cache window and serve any range from the snapshot. Range-scoped
    /// adapters (Google/Graph, which window with `timeMin`/`timeMax`) leave
    /// this `false`, so the host keeps a per-range window. Only consulted on
    /// a full-resync write; ignored on incremental merges.
    #[serde(default)]
    pub complete: bool,
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

    /// Create a new task list (project) at the source and return the
    /// created row. `parent_id` nests it under another list on backends
    /// with nested projects (Vikunja, Todoist); flat backends ignore
    /// it. Default `Unsupported` — adapters that can create lists
    /// override it. The host only routes here for accounts whose
    /// manifest declares the `create_lists` capability.
    async fn create_task_list(&self, _name: &str, _parent_id: Option<&str>) -> Result<TaskList> {
        Err(Error::Unsupported(
            "create_task_list is not supported on this adapter".into(),
        ))
    }

    /// Delete a task list (project) at the source. Default
    /// `Unsupported`; gated on the `delete_lists` capability host-side.
    async fn delete_task_list(&self, _list_id: &str) -> Result<()> {
        Err(Error::Unsupported(
            "delete_task_list is not supported on this adapter".into(),
        ))
    }

    /// Incremental tasks fetch (CACHE-4). Same shape as
    /// [`CalendarFeature::get_events_delta`]; default `Unsupported`.
    async fn get_tasks_delta(
        &self,
        _list_id: &str,
        _since_token: Option<&str>,
    ) -> Result<ChangeSet<Task>> {
        Err(Error::Unsupported(
            "get_tasks_delta is not supported on this adapter".into(),
        ))
    }

    /// Enumerate the users who can be ASSIGNED a task in `list_id` — the
    /// list/project's collaborator pool (Vikunja `projectusers`, Todoist
    /// `collaborators`), NOT the global contacts. Feeds the assignee
    /// picker (§9.7). Default empty: backends without shared lists /
    /// assignment inherit "no one to assign to".
    async fn list_task_list_members(&self, _list_id: &str) -> Result<Vec<TaskUser>> {
        Ok(vec![])
    }

    /// The identity of the connected account itself ("me"), used to tell
    /// "assigned to me" from "assigned to someone else". Fetched once at
    /// connect time and stored on the account. Default `None`: the local
    /// adapter and any provider without a user concept have no remote
    /// identity.
    async fn current_user(&self) -> Result<Option<TaskUser>> {
        Ok(None)
    }

    /// List the EDITABLE membership/shares of `list_id` (Vikunja
    /// `/projects/{id}/users` with rights; Todoist collaborators +
    /// pending invites). Distinct from `list_task_list_members`, which
    /// returns the read-only effective assignee pool. Default empty.
    async fn list_task_list_shares(&self, _list_id: &str) -> Result<Vec<TaskListShare>> {
        Ok(vec![])
    }

    /// Search the instance/workspace for users to add as members
    /// (Vikunja `/users?s=`). Default empty — backends that invite by
    /// raw email (Todoist) have no user directory to search.
    async fn search_users(&self, _query: &str) -> Result<Vec<TaskUser>> {
        Ok(vec![])
    }

    /// Add a member to `list_id`. `member_ref` is the provider's add
    /// key — a username (Vikunja) or an email (Todoist invite). `right`
    /// applies on backends with roles; ignored where unsupported.
    /// Default `Unsupported`.
    async fn add_task_list_member(
        &self,
        _list_id: &str,
        _member_ref: &str,
        _right: Option<MemberRight>,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "add_task_list_member is not supported on this adapter".into(),
        ))
    }

    /// Remove a member from `list_id`. `member_ref` is the provider's
    /// remove key (Vikunja user id, Todoist email). Default
    /// `Unsupported`.
    async fn remove_task_list_member(&self, _list_id: &str, _member_ref: &str) -> Result<()> {
        Err(Error::Unsupported(
            "remove_task_list_member is not supported on this adapter".into(),
        ))
    }

    /// Change an existing member's right (Vikunja). Default
    /// `Unsupported` — backends without per-share roles (Todoist).
    async fn set_task_list_member_right(
        &self,
        _list_id: &str,
        _member_ref: &str,
        _right: MemberRight,
    ) -> Result<()> {
        Err(Error::Unsupported(
            "set_task_list_member_right is not supported on this adapter".into(),
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

    /// Incremental contacts fetch (CACHE-4). Same shape as
    /// [`CalendarFeature::get_events_delta`]; default `Unsupported`.
    async fn get_contacts_delta(
        &self,
        _list_id: &str,
        _since_token: Option<&str>,
    ) -> Result<ChangeSet<Contact>> {
        Err(Error::Unsupported(
            "get_contacts_delta is not supported on this adapter".into(),
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
