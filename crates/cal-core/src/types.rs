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
    pub scheduled_date: Option<NaiveDate>,
    pub deadline_type: Option<DeadlineType>,
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
    pub deadline_type: Option<DeadlineType>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeadlineType {
    /// Specific day + optional time; appears only on the deadline day.
    On,
    /// "Due by"; appears in every configured view from today through the
    /// deadline (section 9.2).
    By,
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
// Contacts
// ────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub display_name: String,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub emails: Vec<String>,
    pub phone_numbers: Vec<String>,
    pub birthday: Option<NaiveDate>,
    pub etag: Option<String>,
}
