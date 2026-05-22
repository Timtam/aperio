//! Data types for calendars, events, tasks, task lists, and contacts.

use base64::Engine;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::color::{ColorLabelId, ContainerColor};
use crate::reminder::{Reminder, SoundConfig};

/// Time interval (half-open: `[start, end)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DateRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl DateRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Self {
        Self { start, end }
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Calendars & events
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    /// Read-only calendars (e.g. birthdays, public holidays, subscribed iCal
    /// feeds) cannot be modified by the caller.
    pub read_only: bool,
    /// Default sound for reminders of all events in this calendar.
    pub default_sound: Option<SoundConfig>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub calendar_id: String,
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub recurrence: Option<EventRecurrence>,
    pub color_label: Option<ColorLabelId>,
    pub reminders: Vec<Reminder>,
    /// Sound override at the event level (section 14.4).
    pub sound: Option<SoundConfig>,
    pub attendees: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Provider ETag / sync tag, used for optimistic-concurrency on push.
    pub etag: Option<String>,
}

/// Payload for creating a new event (without server-assigned IDs).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewEvent {
    pub title: String,
    pub description: Option<String>,
    pub location: Option<String>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub recurrence: Option<EventRecurrence>,
    pub color_label: Option<ColorLabelId>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
    pub attendees: Vec<String>,
}

/// Recurrence rule per RFC 5545 (RRULE).
///
/// Stored as a string so adapters can pass it through verbatim; evaluation
/// happens centrally in the backend with a dedicated crate (Phase 4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventRecurrence {
    pub rrule: String,
    /// Exception dates that should be skipped.
    pub exceptions: Vec<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeBusy {
    pub email: String,
    pub slots: Vec<FreeBusySlot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FreeBusySlot {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

// ────────────────────────────────────────────────────────────────────────────
// Tasks & task lists
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskList {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    pub default_sound: Option<SoundConfig>,
    /// For task-capable calendars (CalDAV/VTODO, local): the calendar ID.
    /// For standalone task lists: `None`.
    pub embedded_in_calendar: Option<String>,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,

    // Scheduling
    //
    // The previous `deadline_type` enum is gone — what used to be
    // `type='on'` is now expressed by setting `scheduled_date` (and
    // optionally `scheduled_time`); what used to be `type='by'` is
    // the only deadline semantic left, expressed via `deadline_date`
    // (+ optional `deadline_time`).
    //
    // A task may have either, both, or neither set. Both means
    // "I plan to do it on `scheduled_date`, and it must be done by
    // `deadline_date`" — the deadline is the backstop, the schedule
    // is the working day.
    //
    // `*_time` fields require their matching `*_date` to be set;
    // the DB enforces this via CHECK constraints (migration 0006).
    pub scheduled_date: Option<NaiveDate>,
    pub scheduled_time: Option<NaiveTime>,
    pub deadline_date: Option<NaiveDate>,
    pub deadline_time: Option<NaiveTime>,

    pub recurrence: Option<TaskRecurrence>,
    pub parent_id: Option<String>,
    pub color_label: Option<ColorLabelId>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub etag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewTask {
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub scheduled_date: Option<NaiveDate>,
    pub scheduled_time: Option<NaiveTime>,
    pub deadline_date: Option<NaiveDate>,
    pub deadline_time: Option<NaiveTime>,
    pub recurrence: Option<TaskRecurrence>,
    pub parent_id: Option<String>,
    pub color_label: Option<ColorLabelId>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Open,
    InProgress,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskPriority {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskRecurrence {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,
    pub day_of_week: Option<Vec<Weekday>>,
    pub day_of_month: Option<u8>,
    pub end: Option<RecurrenceEnd>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecurrenceEnd {
    Never,
    After { occurrences: u32 },
    OnDate { date: NaiveDate },
}

// ────────────────────────────────────────────────────────────────────────────
// Contacts (DESIGN.md §10)
// ────────────────────────────────────────────────────────────────────────────

/// An address book — the contacts equivalent of `Calendar` and `TaskList`.
/// Every `Contact` belongs to exactly one list.
///
/// Sticking the contacts under their own container surface (rather than
/// flattening them under the account) keeps the model coherent with the
/// rest of the app: the sidebar tree still has one section per provider
/// → one node per container → leaf items, and per-list overrides work
/// (e.g. "include in autocomplete" can flip on a list at a time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactList {
    pub id: String,
    pub name: String,
    pub color: Option<ContainerColor>,
    /// Read-only address books (provider-curated lists like "Other
    /// contacts" on Google) can't be modified. Defaults to `false`
    /// for user-owned lists.
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    /// Container reference — the address book this contact lives in.
    pub list_id: String,
    /// Display name as the user wants to see it ("Max Mustermann").
    /// Required: a contact with no display name is a row we wouldn't
    /// know how to render in the picker.
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    /// Organisation / company name. Free-form, multi-valued in
    /// providers like Google but Aperio keeps it scalar for now —
    /// the autocomplete picker only needs the primary value.
    pub organization: Option<String>,
    pub emails: Vec<String>,
    pub phone_numbers: Vec<String>,
    pub birthday: Option<NaiveDate>,
    /// Free-form notes the user (or the provider's UI) attached to
    /// the contact. Surfaced verbatim in the contact dialog; not
    /// indexed for autocomplete.
    pub notes: Option<String>,
    /// Group membership marker (DESIGN.md §10 distribution lists).
    /// `None` ⇒ this is a regular person-contact.
    /// `Some(members)` ⇒ this is a distribution list / address-book
    /// group, with the listed members in declared order. An empty
    /// `Vec` is still a group (a freshly created empty group),
    /// distinguishable from a person via the `Some` wrapper.
    ///
    /// On the wire each provider has its own group convention:
    ///   - EWS uses a separate `<t:DistributionList>` item type
    ///     alongside `<t:Contact>` in the same folder
    ///   - vCard 4.0 uses `KIND:group` + `MEMBER:mailto:…`
    ///   - vCard 3.0 (Apple / older servers) uses
    ///     `X-ADDRESSBOOKSERVER-KIND:group` +
    ///     `X-ADDRESSBOOKSERVER-MEMBER`
    /// — all funnel into this one field so the rest of the stack
    /// stays group-agnostic.
    ///
    /// **Serialization note:** we deliberately keep this field on
    /// the wire even when `None` (no `skip_serializing_if`). The
    /// frontend's group / person discriminator is a `!== null`
    /// check; if serde omitted the field instead, the value
    /// would arrive as `undefined`, satisfy `!== null`, and then
    /// crash on `.members.length`. Keep it explicit.
    #[serde(default)]
    pub members: Option<Vec<GroupMember>>,
    /// Photo presence flag. `true` ⇒ the contact has an avatar
    /// stored on the source (CardDAV PHOTO body, EWS
    /// ContactPicture attachment, local SQLite BLOB). The bytes
    /// themselves are pulled lazily via
    /// `ContactsFeature::get_contact_photo` so listings and the
    /// attendees picker stay cheap — a 1000-contact pull doesn't
    /// haul a megabyte of JPEGs across the wire.
    #[serde(default)]
    pub has_photo: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub etag: Option<String>,
}

/// One member of a distribution list. Mirrors the
/// `<t:Mailbox>` shape EWS uses and the `MEMBER` shape vCard 4.0
/// uses — both encode "(optional display name, email address)" as
/// the canonical identity. We do not link to a concrete `Contact.id`
/// here because the underlying contact can live on a different
/// server, in a different book, or not yet exist as a row at all
/// (typing an email into the picker should still produce a valid
/// member).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupMember {
    /// Display name as the user / provider typed it. Optional —
    /// raw email addresses without a name are common in dumb
    /// CSV imports.
    pub name: Option<String>,
    /// Required: the picker needs an addressable identifier and
    /// vCard's `MEMBER:mailto:` only accepts a URI scheme that
    /// resolves to something. We pin email here.
    pub email: String,
}

/// Payload for creating a new contact. Mirrors `NewTask` / `NewEvent`:
/// no server-assigned ids, no timestamps — the adapter fills those
/// in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewContact {
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub organization: Option<String>,
    pub emails: Vec<String>,
    pub phone_numbers: Vec<String>,
    pub birthday: Option<NaiveDate>,
    pub notes: Option<String>,
    /// See `Contact::members`. `Some(empty vec)` creates a new
    /// distribution list with no initial members; `None` creates a
    /// person-contact (the common case).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub members: Option<Vec<GroupMember>>,
    /// Optional avatar to attach on create. `None` ⇒ no photo;
    /// `Some` ⇒ the adapter writes the bytes through the same
    /// path `set_contact_photo` would. Carrying the photo on
    /// create lets a "new contact with photo" gesture land as a
    /// single command rather than a create-then-upload pair the
    /// caller has to keep transactionally consistent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub photo: Option<ContactPhoto>,
}

/// Binary avatar attached to a contact. Carried inline in
/// `NewContact` on create and shuttled across
/// `ContactsFeature::get_contact_photo` / `set_contact_photo` on
/// read / update; pulled lazily so it doesn't bloat listings.
///
/// `content_type` is a MIME type — we expect `image/jpeg`,
/// `image/png`, or `image/gif` in practice (the three EWS's
/// `ContactPicture.jpg` attachment slot and vCard `PHOTO`
/// property formally permit, and the three the frontend's file
/// picker filters down to).
///
/// `data` is the raw bytes. JSON serialisation uses base64 so the
/// payload travels through the Tauri IPC without ballooning into
/// the giant integer-array shape `Vec<u8>` produces by default —
/// every adapter and the command layer agree on this single
/// encoding via the `serialize_with` / `deserialize_with`
/// helpers below.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactPhoto {
    pub content_type: String,
    #[serde(serialize_with = "serialize_b64", deserialize_with = "deserialize_b64")]
    pub data: Vec<u8>,
}

fn serialize_b64<S: Serializer>(bytes: &[u8], s: S) -> Result<S::Ok, S::Error> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    s.serialize_str(&encoded)
}

fn deserialize_b64<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    base64::engine::general_purpose::STANDARD
        .decode(s.as_bytes())
        .map_err(serde::de::Error::custom)
}
