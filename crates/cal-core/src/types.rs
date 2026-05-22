//! Data types for calendars, events, tasks, task lists, and contacts.

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use serde::{Deserialize, Serialize};

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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub etag: Option<String>,
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
}
