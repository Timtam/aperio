//! UniFFI surface for Aperio's mobile clients.
//!
//! This crate is the *engine-reuse* boundary. The Rust domain logic in
//! [`cal_core`] stays the single source of truth; this thin wrapper re-exports
//! selected pieces of it across an FFI boundary so a Swift (iOS) or Kotlin
//! (Android) UI can call them. The UI is rebuilt per platform — the engine is
//! not. The same generated bindings serve React Native or Flutter, so this is
//! decoupled from the eventual UI choice.
//!
//! cal-core is kept free of any UniFFI dependency. Types that cross the
//! boundary are *mirrored* here as UniFFI records/enums, with `From` /
//! `TryFrom` conversions to and from the core types. Where a core type carries
//! a `chrono::NaiveDate` (which UniFFI has no built-in mapping for), the mirror
//! represents it as an ISO `YYYY-MM-DD` string and parses it back in the
//! conversion — surfacing a [`RecurrenceError`] to the foreign side on bad
//! input rather than panicking.

uniffi::setup_scaffolding!();

/// The full on-device engine handle (accounts + adapter registry over the
/// statically-embedded plugins). Its `#[uniffi::export]` items register
/// themselves with the scaffolding regardless of module visibility.
mod host;

// ───────────────────────────── Attendee parsing ─────────────────────────────

/// A parsed attendee entry: an optional display name plus the email address.
///
/// Mirrors the `(Option<String>, String)` tuple [`cal_core::attendee::parse`]
/// returns — UniFFI needs a named record rather than a bare tuple.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct ParsedAttendee {
    /// Display name, if the entry carried one (`"Jane Doe <jane@host>"`).
    pub name: Option<String>,
    /// The email address (authoritative; taken verbatim from the entry).
    pub email: String,
}

/// Parse a calendar attendee entry into its display name and email.
///
/// Accepts `"Display Name <email@host>"` or a bare `"email@host"`, delegating
/// the split to [`cal_core::attendee::parse`] so the mobile UI and every
/// desktop adapter share one parser.
#[uniffi::export]
pub fn parse_attendee(entry: String) -> ParsedAttendee {
    let (name, email) = cal_core::attendee::parse(&entry);
    ParsedAttendee { name, email }
}

// ───────────────────────── Task recurrence ⇄ RRULE ──────────────────────────

/// How often a recurring task repeats. Mirrors [`cal_core::RecurrenceFrequency`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceFrequency {
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

/// A day of the week (for the weekly `BYDAY` picker). Mirrors [`cal_core::Weekday`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

/// From when the next instance is computed (DESIGN §9.12).
/// Mirrors [`cal_core::RecurrenceAnchor`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrenceAnchor {
    /// Advance from the task's own date.
    FromDate,
    /// Advance from when the task was completed.
    FromCompletion,
}

/// Where a recurring task's next instance is placed (DESIGN §9.12).
/// Mirrors [`cal_core::RecurrencePlacement`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecurrencePlacement {
    /// The next instance gets the computed date.
    Schedule,
    /// The next instance is undated and surfaces in the backlog.
    Backlog,
}

/// A yearless calendar anchor, e.g. `{ month: 4, day: 1 }` for "April 1".
/// Mirrors [`cal_core::MonthDay`].
#[derive(uniffi::Record, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonthDay {
    pub month: u8,
    pub day: u8,
}

/// When a recurrence stops. Mirrors [`cal_core::RecurrenceEnd`], except the
/// `OnDate` date is an ISO `YYYY-MM-DD` string on the FFI boundary (UniFFI has
/// no built-in date type); it is parsed back to a real date when converted into
/// the core model.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum RecurrenceEnd {
    /// Repeats forever.
    Never,
    /// Stops after a fixed number of occurrences (`COUNT`).
    After { occurrences: u32 },
    /// Stops on a date (`UNTIL`), as `YYYY-MM-DD`.
    OnDate { date: String },
}

/// Structured task recurrence. Mirrors [`cal_core::TaskRecurrence`] (with the
/// `UNTIL` date represented as a string — see [`RecurrenceEnd`]).
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct TaskRecurrence {
    pub frequency: RecurrenceFrequency,
    pub interval: u32,
    pub day_of_week: Option<Vec<Weekday>>,
    pub day_of_month: Option<u8>,
    pub end: Option<RecurrenceEnd>,
    pub anchor: RecurrenceAnchor,
    pub placement: RecurrencePlacement,
    pub fixed_dates: Option<Vec<MonthDay>>,
}

/// Error returned when a [`TaskRecurrence`] coming from the foreign side cannot
/// be turned into the core model.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum RecurrenceError {
    /// The `OnDate` end carried a string that is not a valid `YYYY-MM-DD` date.
    #[error("invalid UNTIL date '{date}', expected YYYY-MM-DD")]
    InvalidDate { date: String },
}

/// The ISO date format used for dates across the boundary (`RecurrenceEnd::OnDate`,
/// task scheduling, …).
const DATE_FMT: &str = "%Y-%m-%d";

/// The clock-time format used for task `*_time` fields across the boundary.
const TIME_FMT: &str = "%H:%M:%S";

/// Serialize a [`TaskRecurrence`] into an RFC 5545 `RRULE` value (without the
/// `RRULE:` prefix), e.g. `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE`.
///
/// Errors only if an `OnDate` end carries an unparseable date string.
#[uniffi::export]
pub fn task_recurrence_to_rrule(recurrence: TaskRecurrence) -> Result<String, RecurrenceError> {
    let core: cal_core::TaskRecurrence = recurrence.try_into()?;
    Ok(cal_core::task_recurrence_to_rrule(&core))
}

/// Parse an RFC 5545 `RRULE` value into a [`TaskRecurrence`]. Tolerates a
/// leading `RRULE:` and surrounding whitespace; returns `None` when there is no
/// usable `FREQ`. Unmodelled parts are ignored.
#[uniffi::export]
pub fn rrule_to_task_recurrence(rrule: String) -> Option<TaskRecurrence> {
    cal_core::rrule_to_task_recurrence(&rrule).map(TaskRecurrence::from)
}

// ─────────────────────────── core -> ffi (infallible) ───────────────────────

impl From<cal_core::RecurrenceFrequency> for RecurrenceFrequency {
    fn from(f: cal_core::RecurrenceFrequency) -> Self {
        use cal_core::RecurrenceFrequency as C;
        match f {
            C::Daily => Self::Daily,
            C::Weekly => Self::Weekly,
            C::Monthly => Self::Monthly,
            C::Yearly => Self::Yearly,
        }
    }
}

impl From<cal_core::Weekday> for Weekday {
    fn from(w: cal_core::Weekday) -> Self {
        use cal_core::Weekday as C;
        match w {
            C::Monday => Self::Monday,
            C::Tuesday => Self::Tuesday,
            C::Wednesday => Self::Wednesday,
            C::Thursday => Self::Thursday,
            C::Friday => Self::Friday,
            C::Saturday => Self::Saturday,
            C::Sunday => Self::Sunday,
        }
    }
}

impl From<cal_core::RecurrenceAnchor> for RecurrenceAnchor {
    fn from(a: cal_core::RecurrenceAnchor) -> Self {
        use cal_core::RecurrenceAnchor as C;
        match a {
            C::FromDate => Self::FromDate,
            C::FromCompletion => Self::FromCompletion,
        }
    }
}

impl From<cal_core::RecurrencePlacement> for RecurrencePlacement {
    fn from(p: cal_core::RecurrencePlacement) -> Self {
        use cal_core::RecurrencePlacement as C;
        match p {
            C::Schedule => Self::Schedule,
            C::Backlog => Self::Backlog,
        }
    }
}

impl From<cal_core::MonthDay> for MonthDay {
    fn from(md: cal_core::MonthDay) -> Self {
        Self {
            month: md.month,
            day: md.day,
        }
    }
}

impl From<cal_core::RecurrenceEnd> for RecurrenceEnd {
    fn from(e: cal_core::RecurrenceEnd) -> Self {
        use cal_core::RecurrenceEnd as C;
        match e {
            C::Never => Self::Never,
            C::After { occurrences } => Self::After { occurrences },
            C::OnDate { date } => Self::OnDate {
                date: date.format(DATE_FMT).to_string(),
            },
        }
    }
}

impl From<cal_core::TaskRecurrence> for TaskRecurrence {
    fn from(r: cal_core::TaskRecurrence) -> Self {
        Self {
            frequency: r.frequency.into(),
            interval: r.interval,
            day_of_week: r
                .day_of_week
                .map(|days| days.into_iter().map(Weekday::from).collect()),
            day_of_month: r.day_of_month,
            end: r.end.map(RecurrenceEnd::from),
            anchor: r.anchor.into(),
            placement: r.placement.into(),
            fixed_dates: r
                .fixed_dates
                .map(|days| days.into_iter().map(MonthDay::from).collect()),
        }
    }
}

// ──────────────────── ffi -> core (fallible where dates appear) ──────────────

impl From<RecurrenceFrequency> for cal_core::RecurrenceFrequency {
    fn from(f: RecurrenceFrequency) -> Self {
        match f {
            RecurrenceFrequency::Daily => Self::Daily,
            RecurrenceFrequency::Weekly => Self::Weekly,
            RecurrenceFrequency::Monthly => Self::Monthly,
            RecurrenceFrequency::Yearly => Self::Yearly,
        }
    }
}

impl From<Weekday> for cal_core::Weekday {
    fn from(w: Weekday) -> Self {
        match w {
            Weekday::Monday => Self::Monday,
            Weekday::Tuesday => Self::Tuesday,
            Weekday::Wednesday => Self::Wednesday,
            Weekday::Thursday => Self::Thursday,
            Weekday::Friday => Self::Friday,
            Weekday::Saturday => Self::Saturday,
            Weekday::Sunday => Self::Sunday,
        }
    }
}

impl From<RecurrenceAnchor> for cal_core::RecurrenceAnchor {
    fn from(a: RecurrenceAnchor) -> Self {
        match a {
            RecurrenceAnchor::FromDate => Self::FromDate,
            RecurrenceAnchor::FromCompletion => Self::FromCompletion,
        }
    }
}

impl From<RecurrencePlacement> for cal_core::RecurrencePlacement {
    fn from(p: RecurrencePlacement) -> Self {
        match p {
            RecurrencePlacement::Schedule => Self::Schedule,
            RecurrencePlacement::Backlog => Self::Backlog,
        }
    }
}

impl From<MonthDay> for cal_core::MonthDay {
    fn from(md: MonthDay) -> Self {
        Self {
            month: md.month,
            day: md.day,
        }
    }
}

impl TryFrom<RecurrenceEnd> for cal_core::RecurrenceEnd {
    type Error = RecurrenceError;

    fn try_from(e: RecurrenceEnd) -> Result<Self, Self::Error> {
        Ok(match e {
            RecurrenceEnd::Never => Self::Never,
            RecurrenceEnd::After { occurrences } => Self::After { occurrences },
            RecurrenceEnd::OnDate { date } => {
                let parsed = chrono::NaiveDate::parse_from_str(&date, DATE_FMT)
                    .map_err(|_| RecurrenceError::InvalidDate { date })?;
                Self::OnDate { date: parsed }
            }
        })
    }
}

impl TryFrom<TaskRecurrence> for cal_core::TaskRecurrence {
    type Error = RecurrenceError;

    fn try_from(r: TaskRecurrence) -> Result<Self, Self::Error> {
        Ok(Self {
            frequency: r.frequency.into(),
            interval: r.interval,
            day_of_week: r
                .day_of_week
                .map(|days| days.into_iter().map(cal_core::Weekday::from).collect()),
            day_of_month: r.day_of_month,
            end: r.end.map(cal_core::RecurrenceEnd::try_from).transpose()?,
            anchor: r.anchor.into(),
            placement: r.placement.into(),
            fixed_dates: r
                .fixed_dates
                .map(|days| days.into_iter().map(cal_core::MonthDay::from).collect()),
        })
    }
}

// ─────────────────────────── Task status & priority ─────────────────────────

/// Lifecycle state of a task. Mirrors [`cal_core::TaskStatus`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Open,
    InProgress,
    Completed,
    Cancelled,
}

impl From<cal_core::TaskStatus> for TaskStatus {
    fn from(s: cal_core::TaskStatus) -> Self {
        use cal_core::TaskStatus as C;
        match s {
            C::Open => Self::Open,
            C::InProgress => Self::InProgress,
            C::Completed => Self::Completed,
            C::Cancelled => Self::Cancelled,
        }
    }
}

impl From<TaskStatus> for cal_core::TaskStatus {
    fn from(s: TaskStatus) -> Self {
        match s {
            TaskStatus::Open => Self::Open,
            TaskStatus::InProgress => Self::InProgress,
            TaskStatus::Completed => Self::Completed,
            TaskStatus::Cancelled => Self::Cancelled,
        }
    }
}

/// Task priority. Mirrors [`cal_core::TaskPriority`].
#[derive(uniffi::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPriority {
    Low,
    Medium,
    High,
}

impl From<cal_core::TaskPriority> for TaskPriority {
    fn from(p: cal_core::TaskPriority) -> Self {
        use cal_core::TaskPriority as C;
        match p {
            C::Low => Self::Low,
            C::Medium => Self::Medium,
            C::High => Self::High,
        }
    }
}

impl From<TaskPriority> for cal_core::TaskPriority {
    fn from(p: TaskPriority) -> Self {
        match p {
            TaskPriority::Low => Self::Low,
            TaskPriority::Medium => Self::Medium,
            TaskPriority::High => Self::High,
        }
    }
}

// ───────────────────────────── Reminders & sound ────────────────────────────

/// Where a notification sound comes from. Mirrors [`cal_core::SoundSource`].
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum SoundSource {
    /// Platform default notification sound.
    System,
    /// Silent (visual only).
    Silent,
    /// User-supplied audio file, referenced by its content hash.
    Custom { sha256: String },
}

impl From<cal_core::SoundSource> for SoundSource {
    fn from(s: cal_core::SoundSource) -> Self {
        use cal_core::SoundSource as C;
        match s {
            C::System => Self::System,
            C::Silent => Self::Silent,
            C::Custom { sha256 } => Self::Custom { sha256 },
        }
    }
}

impl From<SoundSource> for cal_core::SoundSource {
    fn from(s: SoundSource) -> Self {
        match s {
            SoundSource::System => Self::System,
            SoundSource::Silent => Self::Silent,
            SoundSource::Custom { sha256 } => Self::Custom { sha256 },
        }
    }
}

/// Notification sound configuration. Mirrors [`cal_core::SoundConfig`].
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct SoundConfig {
    pub source: SoundSource,
    /// Volume 0–100, independent of the system volume.
    pub volume: u8,
}

impl From<cal_core::SoundConfig> for SoundConfig {
    fn from(s: cal_core::SoundConfig) -> Self {
        Self {
            source: s.source.into(),
            volume: s.volume,
        }
    }
}

impl From<SoundConfig> for cal_core::SoundConfig {
    fn from(s: SoundConfig) -> Self {
        Self {
            source: s.source.into(),
            volume: s.volume,
        }
    }
}

/// A reminder trigger. Mirrors [`cal_core::ReminderKind`], with the
/// `Absolute` instant represented as an RFC 3339 string on the boundary.
#[derive(uniffi::Enum, Debug, Clone, PartialEq, Eq)]
pub enum ReminderKind {
    /// Relative to the task's deadline; `minutes_before` may be negative to
    /// fire after the reference time.
    Relative { minutes_before: i64 },
    /// Fixed point in time (RFC 3339, e.g. `2026-06-16T09:00:00+00:00`).
    Absolute { at: String },
    /// Fires on the next app start after the due time.
    AppStart,
    /// E-mail reminder.
    Email { minutes_before: i64 },
}

impl From<cal_core::ReminderKind> for ReminderKind {
    fn from(k: cal_core::ReminderKind) -> Self {
        use cal_core::ReminderKind as C;
        match k {
            C::Relative { minutes_before } => Self::Relative { minutes_before },
            C::Absolute { at } => Self::Absolute {
                at: at.to_rfc3339(),
            },
            C::AppStart => Self::AppStart,
            C::Email { minutes_before } => Self::Email { minutes_before },
        }
    }
}

impl TryFrom<ReminderKind> for cal_core::ReminderKind {
    type Error = StoreError;

    fn try_from(k: ReminderKind) -> Result<Self, Self::Error> {
        Ok(match k {
            ReminderKind::Relative { minutes_before } => Self::Relative { minutes_before },
            ReminderKind::Absolute { at } => Self::Absolute {
                at: parse_utc_field("reminder.at", &at)?,
            },
            ReminderKind::AppStart => Self::AppStart,
            ReminderKind::Email { minutes_before } => Self::Email { minutes_before },
        })
    }
}

/// A reminder attached to a task. Mirrors [`cal_core::Reminder`].
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    pub kind: ReminderKind,
    /// Overrides the task-level sound when set.
    pub sound: Option<SoundConfig>,
}

impl From<cal_core::Reminder> for Reminder {
    fn from(r: cal_core::Reminder) -> Self {
        Self {
            kind: r.kind.into(),
            sound: r.sound.map(SoundConfig::from),
        }
    }
}

impl TryFrom<Reminder> for cal_core::Reminder {
    type Error = StoreError;

    fn try_from(r: Reminder) -> Result<Self, Self::Error> {
        Ok(Self {
            kind: r.kind.try_into()?,
            sound: r.sound.map(cal_core::SoundConfig::from),
        })
    }
}

// ─────────────────────────────── Local store ────────────────────────────────
//
// The on-device data layer. Opens the app-sandbox SQLite (migrated by the
// shared `aperio-db` runner) and serves task CRUD through the same
// `cal_adapter_local::LocalAdapter` the desktop backend uses — engine reuse,
// not a re-implementation.

use std::sync::{Arc, Mutex};

use cal_adapter_local::LocalAdapter;
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

/// A task list as it crosses the FFI boundary. Minimal first slice —
/// richer fields (color, sound, calendar binding) follow as the UI needs
/// them.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct TaskListDto {
    pub id: String,
    pub name: String,
    /// Parent project id for nested backends; `None` for a top-level list.
    pub parent_id: Option<String>,
    pub read_only: bool,
}

impl From<cal_core::TaskList> for TaskListDto {
    fn from(l: cal_core::TaskList) -> Self {
        Self {
            id: l.id,
            name: l.name,
            parent_id: l.parent_id,
            read_only: l.read_only,
        }
    }
}

/// A task as it crosses the FFI boundary — a lossless mirror of
/// [`cal_core::Task`] for the on-device store. Dates are ISO `YYYY-MM-DD`,
/// times are `HH:MM:SS`, and instants are RFC 3339 strings (UniFFI has no
/// built-in date/time types). Assignees are intentionally omitted: the local
/// store does not persist them (a sync-era, multi-user concept), so surfacing
/// them here would be misleading.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct TaskDto {
    pub id: String,
    pub list_id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    /// `YYYY-MM-DD`. The day the task is planned for.
    pub scheduled_date: Option<String>,
    /// `HH:MM:SS`. Requires `scheduled_date`.
    pub scheduled_time: Option<String>,
    /// `YYYY-MM-DD`. The day the task is due by.
    pub deadline_date: Option<String>,
    /// `HH:MM:SS`. Requires `deadline_date`.
    pub deadline_time: Option<String>,
    pub recurrence: Option<TaskRecurrence>,
    /// `YYYY-MM-DD`. Backlog resurface trigger (DESIGN §9.12), derived by the
    /// recurrence engine when a backlog instance is spawned. Round-trips
    /// through [`LocalStore::update_task`].
    pub resurface_date: Option<String>,
    /// Stable id of the recurring series this instance belongs to. The store
    /// assigns it when a recurring task is created; pass it back unchanged on
    /// update (it round-trips through [`LocalStore::update_task`]).
    pub series_id: Option<String>,
    pub parent_id: Option<String>,
    pub section_id: Option<String>,
    /// Global color-label id, if bound.
    pub color_label: Option<String>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
    /// RFC 3339, set once the task is completed.
    pub completed_at: Option<String>,
    pub etag: Option<String>,
}

impl From<cal_core::Task> for TaskDto {
    fn from(t: cal_core::Task) -> Self {
        Self {
            id: t.id,
            list_id: t.list_id,
            title: t.title,
            description: t.description,
            status: t.status.into(),
            priority: t.priority.into(),
            scheduled_date: t.scheduled_date.map(date_to_string),
            scheduled_time: t.scheduled_time.map(time_to_string),
            deadline_date: t.deadline_date.map(date_to_string),
            deadline_time: t.deadline_time.map(time_to_string),
            recurrence: t.recurrence.map(TaskRecurrence::from),
            resurface_date: t.resurface_date.map(date_to_string),
            series_id: t.series_id,
            parent_id: t.parent_id,
            section_id: t.section_id,
            color_label: t.color_label.map(|c| c.0),
            reminders: t.reminders.into_iter().map(Reminder::from).collect(),
            sound: t.sound.map(SoundConfig::from),
            created_at: utc_to_string(t.created_at),
            updated_at: utc_to_string(t.updated_at),
            completed_at: t.completed_at.map(utc_to_string),
            etag: t.etag,
        }
    }
}

impl TryFrom<TaskDto> for cal_core::Task {
    type Error = StoreError;

    fn try_from(t: TaskDto) -> Result<Self, Self::Error> {
        Ok(Self {
            id: t.id,
            list_id: t.list_id,
            title: t.title,
            description: t.description,
            status: t.status.into(),
            priority: t.priority.into(),
            scheduled_date: opt_date_field("scheduled_date", t.scheduled_date)?,
            scheduled_time: opt_time_field("scheduled_time", t.scheduled_time)?,
            deadline_date: opt_date_field("deadline_date", t.deadline_date)?,
            deadline_time: opt_time_field("deadline_time", t.deadline_time)?,
            recurrence: t
                .recurrence
                .map(cal_core::TaskRecurrence::try_from)
                .transpose()?,
            resurface_date: opt_date_field("resurface_date", t.resurface_date)?,
            series_id: t.series_id,
            parent_id: t.parent_id,
            section_id: t.section_id,
            color_label: t.color_label.map(cal_core::ColorLabelId),
            reminders: t
                .reminders
                .into_iter()
                .map(cal_core::Reminder::try_from)
                .collect::<Result<_, _>>()?,
            sound: t.sound.map(cal_core::SoundConfig::from),
            assignees: Vec::new(),
            created_at: parse_utc_field("created_at", &t.created_at)?,
            updated_at: parse_utc_field("updated_at", &t.updated_at)?,
            completed_at: opt_utc_field("completed_at", t.completed_at)?,
            etag: t.etag,
        })
    }
}

/// The editable shape for creating a task — a mirror of [`cal_core::NewTask`]
/// (no id/timestamps/etag, no assignees; see [`TaskDto`] for the conventions).
///
/// `series_id` and `resurface_date` are deliberately omitted: they are
/// store-managed (the local adapter assigns a `series_id` to a recurring task
/// on create, and derives `resurface_date` when it spawns a backlog instance —
/// DESIGN §9.12), not values a client sets.
#[derive(uniffi::Record, Debug, Clone, PartialEq, Eq)]
pub struct NewTaskDto {
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub priority: TaskPriority,
    pub scheduled_date: Option<String>,
    pub scheduled_time: Option<String>,
    pub deadline_date: Option<String>,
    pub deadline_time: Option<String>,
    pub recurrence: Option<TaskRecurrence>,
    pub parent_id: Option<String>,
    pub section_id: Option<String>,
    pub color_label: Option<String>,
    pub reminders: Vec<Reminder>,
    pub sound: Option<SoundConfig>,
}

impl TryFrom<NewTaskDto> for cal_core::NewTask {
    type Error = StoreError;

    fn try_from(t: NewTaskDto) -> Result<Self, Self::Error> {
        Ok(Self {
            title: t.title,
            description: t.description,
            status: t.status.into(),
            priority: t.priority.into(),
            scheduled_date: opt_date_field("scheduled_date", t.scheduled_date)?,
            scheduled_time: opt_time_field("scheduled_time", t.scheduled_time)?,
            deadline_date: opt_date_field("deadline_date", t.deadline_date)?,
            deadline_time: opt_time_field("deadline_time", t.deadline_time)?,
            recurrence: t
                .recurrence
                .map(cal_core::TaskRecurrence::try_from)
                .transpose()?,
            // Store-managed (see the type's docs): the adapter assigns a
            // series_id to a recurring task on create and derives
            // resurface_date for spawned backlog instances.
            resurface_date: None,
            series_id: None,
            parent_id: t.parent_id,
            section_id: t.section_id,
            color_label: t.color_label.map(cal_core::ColorLabelId),
            reminders: t
                .reminders
                .into_iter()
                .map(cal_core::Reminder::try_from)
                .collect::<Result<_, _>>()?,
            sound: t.sound.map(cal_core::SoundConfig::from),
            assignees: Vec::new(),
        })
    }
}

/// Errors surfaced from the on-device store to the foreign side.
///
/// The variants mirror the desktop's `CommandError` codes (the `From<
/// cal_core::Error>` mapping in `src-tauri/src/commands/mod.rs`) so the mobile
/// UI can branch on the same distinctions — re-auth on `Auth`, an
/// optimistic-concurrency retry on `Conflict`, a transient banner on
/// `Network`, etc. — instead of getting one opaque storage error. The
/// external-adapter event paths are where the full spread becomes reachable.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum StoreError {
    /// Opening or migrating the database file failed.
    #[error("could not open the local database: {detail}")]
    Open { detail: String },
    /// A read or write against the local database (or an unclassified adapter
    /// failure) failed.
    #[error("storage error: {detail}")]
    Storage { detail: String },
    /// The requested row does not exist.
    #[error("not found")]
    NotFound,
    /// A value coming from the foreign side could not be parsed into the
    /// core model (a malformed date, time, datetime, recurrence rule, …) or
    /// was otherwise rejected as invalid input.
    #[error("invalid value for {field}: {detail}")]
    InvalidField { field: String, detail: String },
    /// The adapter rejected the credentials (expired / wrong token) — the UI
    /// surfaces the re-connect flow.
    #[error("authentication failed: {detail}")]
    Auth { detail: String },
    /// The account is authenticated but not allowed to perform the operation.
    #[error("access denied: {detail}")]
    Forbidden { detail: String },
    /// An ETag / precondition-failed clash (the row changed underneath us) —
    /// the UI re-reads and retries.
    #[error("conflict: {detail}")]
    Conflict { detail: String },
    /// A transient network failure reaching the provider.
    #[error("network error: {detail}")]
    Network { detail: String },
    /// The provider answered with something the adapter couldn't parse.
    #[error("protocol error: {detail}")]
    Protocol { detail: String },
    /// The adapter doesn't support this operation.
    #[error("operation not supported: {detail}")]
    Unsupported { detail: String },
}

impl From<RecurrenceError> for StoreError {
    fn from(e: RecurrenceError) -> Self {
        match e {
            RecurrenceError::InvalidDate { date } => StoreError::InvalidField {
                field: "recurrence".to_string(),
                detail: format!("invalid UNTIL date '{date}', expected YYYY-MM-DD"),
            },
        }
    }
}

/// Map a core error from the adapter to the FFI store error, preserving every
/// distinction the desktop's `CommandError` keeps (the UI branches on these —
/// re-auth, conflict-retry, network banner). Exhaustive on purpose: a new
/// `cal_core::Error` variant forces a compile error here rather than silently
/// collapsing into `Storage`.
fn map_store_err(e: cal_core::Error) -> StoreError {
    use cal_core::Error as E;
    match e {
        E::NotFound(_) => StoreError::NotFound,
        E::InvalidInput(detail) => StoreError::InvalidField {
            field: "input".to_string(),
            detail,
        },
        E::Authentication(detail) => StoreError::Auth { detail },
        E::Forbidden(detail) => StoreError::Forbidden { detail },
        E::Conflict(detail) => StoreError::Conflict { detail },
        E::Network(detail) => StoreError::Network { detail },
        E::Protocol(detail) => StoreError::Protocol { detail },
        E::Unsupported(detail) => StoreError::Unsupported { detail },
        E::Internal(detail) => StoreError::Storage { detail },
    }
}

// ── Boundary date/time helpers ───────────────────────────────────────────────
//
// Dates cross as `YYYY-MM-DD`, clock times as `HH:MM:SS`, and instants as
// RFC 3339 — the same shapes the desktop SQLite layer stores, kept consistent
// so the formats never drift between the two backends.

fn date_to_string(d: NaiveDate) -> String {
    d.format(DATE_FMT).to_string()
}

fn time_to_string(t: NaiveTime) -> String {
    t.format(TIME_FMT).to_string()
}

fn utc_to_string(t: DateTime<Utc>) -> String {
    t.to_rfc3339()
}

fn opt_date_field(field: &str, s: Option<String>) -> Result<Option<NaiveDate>, StoreError> {
    s.map(|s| {
        NaiveDate::parse_from_str(&s, DATE_FMT).map_err(|_| StoreError::InvalidField {
            field: field.to_string(),
            detail: format!("invalid date '{s}', expected YYYY-MM-DD"),
        })
    })
    .transpose()
}

fn opt_time_field(field: &str, s: Option<String>) -> Result<Option<NaiveTime>, StoreError> {
    s.map(|s| {
        NaiveTime::parse_from_str(&s, TIME_FMT).map_err(|_| StoreError::InvalidField {
            field: field.to_string(),
            detail: format!("invalid time '{s}', expected HH:MM:SS"),
        })
    })
    .transpose()
}

fn parse_utc_field(field: &str, s: &str) -> Result<DateTime<Utc>, StoreError> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|_| StoreError::InvalidField {
            field: field.to_string(),
            detail: format!("invalid datetime '{s}', expected RFC 3339"),
        })
}

fn opt_utc_field(field: &str, s: Option<String>) -> Result<Option<DateTime<Utc>>, StoreError> {
    s.map(|s| parse_utc_field(field, &s)).transpose()
}

/// Serialize a value to the JSON the bridge hands to JS — the `cal_core` serde
/// shape, identical to the desktop's Tauri payloads.
fn to_json<T: serde::Serialize>(value: &T) -> Result<String, StoreError> {
    serde_json::to_string(value).map_err(|e| StoreError::Storage {
        detail: format!("serialize: {e}"),
    })
}

/// Parse JSON from the foreign side into a `cal_core` value.
fn from_json<T: serde::de::DeserializeOwned>(field: &str, json: &str) -> Result<T, StoreError> {
    serde_json::from_str(json).map_err(|e| StoreError::InvalidField {
        field: field.to_string(),
        detail: format!("invalid JSON: {e}"),
    })
}

/// The mobile app's handle to its on-device SQLite store.
///
/// Opens (and migrates, via the shared [`aperio_db`] runner) the database
/// at `db_path`, then serves task CRUD through the same
/// [`cal_adapter_local::LocalAdapter`] the desktop backend uses. The UI
/// holds one instance per launch and passes an app-sandbox path (e.g.
/// `<Documents>/aperio.sqlite`).
#[derive(uniffi::Object)]
pub struct LocalStore {
    adapter: LocalAdapter,
}

#[uniffi::export]
impl LocalStore {
    /// Open the on-device database at `db_path`, creating the file and
    /// applying any pending migrations, and bind a local adapter to it.
    #[uniffi::constructor]
    pub fn open(db_path: String) -> Result<Arc<Self>, StoreError> {
        let mut conn = rusqlite::Connection::open(&db_path).map_err(|e| StoreError::Open {
            detail: e.to_string(),
        })?;
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;",
        )
        .map_err(|e| StoreError::Open {
            detail: e.to_string(),
        })?;
        aperio_db::run(&mut conn).map_err(|e| StoreError::Open {
            detail: e.to_string(),
        })?;
        let shared = Arc::new(Mutex::new(conn));
        Ok(Arc::new(Self {
            adapter: LocalAdapter::new(shared),
        }))
    }

    /// Create a new local, top-level task list and return it.
    pub fn create_task_list(&self, name: String) -> Result<TaskListDto, StoreError> {
        self.adapter
            .create_task_list(&name, None, None, None, None)
            .map(TaskListDto::from)
            .map_err(map_store_err)
    }

    /// Fetch a task list by id; [`StoreError::NotFound`] when absent.
    pub fn task_list(&self, id: String) -> Result<TaskListDto, StoreError> {
        self.adapter
            .get_task_list_by_id(&id)
            .map_err(map_store_err)?
            .map(TaskListDto::from)
            .ok_or(StoreError::NotFound)
    }

    /// List all task lists, ordered by name (case-insensitive).
    pub fn task_lists(&self) -> Result<Vec<TaskListDto>, StoreError> {
        self.adapter
            .list_task_lists_sync()
            .map(|lists| lists.into_iter().map(TaskListDto::from).collect())
            .map_err(map_store_err)
    }

    /// Rename a task list. Rejects an empty/whitespace-only name
    /// ([`StoreError::InvalidField`]); [`StoreError::NotFound`] for an
    /// unknown id.
    pub fn rename_task_list(&self, id: String, new_name: String) -> Result<(), StoreError> {
        self.adapter
            .rename_task_list_sync(&id, &new_name)
            .map_err(map_store_err)
    }

    /// Delete a task list; its tasks cascade away.
    /// [`StoreError::NotFound`] when the id is unknown.
    pub fn delete_task_list(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_task_list(&id).map_err(map_store_err)
    }

    /// List the tasks in a list, ordered by date then creation time.
    pub fn tasks(&self, list_id: String) -> Result<Vec<TaskDto>, StoreError> {
        self.adapter
            .get_tasks_sync(&list_id)
            .map(|tasks| tasks.into_iter().map(TaskDto::from).collect())
            .map_err(map_store_err)
    }

    /// Fetch a single task by id; [`StoreError::NotFound`] when absent.
    pub fn task(&self, id: String) -> Result<TaskDto, StoreError> {
        self.adapter
            .get_task_by_id(&id)
            .map_err(map_store_err)?
            .map(TaskDto::from)
            .ok_or(StoreError::NotFound)
    }

    /// Create a task in `list_id` and return it (with its assigned id and
    /// timestamps). A recurring task gets a stable `series_id` (DESIGN §9.12).
    pub fn create_task(&self, list_id: String, task: NewTaskDto) -> Result<TaskDto, StoreError> {
        let new: cal_core::NewTask = task.try_into()?;
        self.adapter
            .create_task_sync(&list_id, new)
            .map(TaskDto::from)
            .map_err(map_store_err)
    }

    /// Update a task — a full overwrite of its mutable fields (everything but
    /// the immutable `created_at`), so a faithful read-modify-write round-trip
    /// preserves `series_id` and `resurface_date` by passing them back as read.
    /// Completing a recurring task spawns its next instance, which shows up on
    /// the next [`LocalStore::tasks`] call (DESIGN §9.12). [`StoreError::NotFound`]
    /// when the id is unknown.
    pub fn update_task(&self, task: TaskDto) -> Result<TaskDto, StoreError> {
        let core: cal_core::Task = task.try_into()?;
        self.adapter
            .update_task_sync(core)
            .map(TaskDto::from)
            .map_err(map_store_err)
    }

    /// Delete a task. [`StoreError::NotFound`] when the id is unknown.
    pub fn delete_task(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_task_sync(&id).map_err(map_store_err)
    }

    // ── JSON bridge surface (the faithful tasks port) ────────────────────────
    //
    // The full task/list/section domain crosses as JSON in the `cal_core` serde
    // shape — identical to the desktop's Tauri payloads — so the hand-written
    // mobile native module is a trivial string passthrough and the shared TS
    // domain logic (types, grouping, labels) is reused verbatim. The typed DTO
    // methods above stay for direct/typed consumers and the tests.

    /// All task lists as a JSON array (`cal_core::TaskList[]`).
    pub fn task_lists_json(&self) -> Result<String, StoreError> {
        let lists = self.adapter.list_task_lists_sync().map_err(map_store_err)?;
        to_json(&lists)
    }

    /// Create a top-level local task list; returns the created `TaskList` as JSON.
    pub fn create_task_list_json(&self, name: String) -> Result<String, StoreError> {
        let list = self
            .adapter
            .create_task_list(&name, None, None, None, None)
            .map_err(map_store_err)?;
        to_json(&list)
    }

    /// Set or clear a list's parent (`parent_id = None` promotes to top level);
    /// returns the updated `TaskList` as JSON.
    pub fn reparent_task_list_json(
        &self,
        id: String,
        parent_id: Option<String>,
    ) -> Result<String, StoreError> {
        let list = self
            .adapter
            .reparent_task_list(&id, parent_id.as_deref())
            .map_err(map_store_err)?;
        to_json(&list)
    }

    /// Tasks in a list as a JSON array (`cal_core::Task[]`), ordered by date
    /// then creation time.
    pub fn tasks_json(&self, list_id: String) -> Result<String, StoreError> {
        let tasks = self
            .adapter
            .get_tasks_sync(&list_id)
            .map_err(map_store_err)?;
        to_json(&tasks)
    }

    /// One task by id as JSON; [`StoreError::NotFound`] when absent.
    pub fn task_json(&self, id: String) -> Result<String, StoreError> {
        let task = self
            .adapter
            .get_task_by_id(&id)
            .map_err(map_store_err)?
            .ok_or(StoreError::NotFound)?;
        to_json(&task)
    }

    /// Create a task from a JSON `cal_core::NewTask`; returns the created `Task`
    /// as JSON (a recurring task is assigned a stable series id).
    pub fn create_task_json(
        &self,
        list_id: String,
        new_task_json: String,
    ) -> Result<String, StoreError> {
        let new: cal_core::NewTask = from_json("task", &new_task_json)?;
        let task = self
            .adapter
            .create_task_sync(&list_id, new)
            .map_err(map_store_err)?;
        to_json(&task)
    }

    /// Update a task from a JSON `cal_core::Task`; returns the updated `Task` as
    /// JSON. Completing a recurring task spawns its next instance (DESIGN §9.12).
    pub fn update_task_json(&self, task_json: String) -> Result<String, StoreError> {
        let task: cal_core::Task = from_json("task", &task_json)?;
        let updated = self.adapter.update_task_sync(task).map_err(map_store_err)?;
        to_json(&updated)
    }

    /// Sections of a list as a JSON array (`cal_core::Section[]`), ordered by
    /// position then name.
    pub fn sections_json(&self, list_id: String) -> Result<String, StoreError> {
        let sections = self
            .adapter
            .list_sections_sync(&list_id)
            .map_err(map_store_err)?;
        to_json(&sections)
    }

    /// Create a section in a list; returns the created `Section` as JSON.
    pub fn create_section_json(
        &self,
        list_id: String,
        name: String,
        position: u32,
        color_label: Option<String>,
    ) -> Result<String, StoreError> {
        let section = self
            .adapter
            .create_section(
                &list_id,
                &name,
                position,
                color_label.map(cal_core::ColorLabelId),
            )
            .map_err(map_store_err)?;
        to_json(&section)
    }

    /// Update a section from a JSON `cal_core::Section`; returns it as JSON.
    pub fn update_section_json(&self, section_json: String) -> Result<String, StoreError> {
        let section: cal_core::Section = from_json("section", &section_json)?;
        let updated = self
            .adapter
            .update_section(section)
            .map_err(map_store_err)?;
        to_json(&updated)
    }

    /// Delete a section; its tasks fall back to ungrouped (`section_id` → NULL).
    pub fn delete_section(&self, id: String) -> Result<(), StoreError> {
        self.adapter.delete_section(&id).map_err(map_store_err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_attendee_wraps_core_parser_for_named_entry() {
        assert_eq!(
            parse_attendee("Alice Smith <alice@example.com>".to_string()),
            ParsedAttendee {
                name: Some("Alice Smith".to_string()),
                email: "alice@example.com".to_string(),
            }
        );
    }

    #[test]
    fn parse_attendee_bare_email_has_no_name() {
        assert_eq!(
            parse_attendee("bob@example.com".to_string()),
            ParsedAttendee {
                name: None,
                email: "bob@example.com".to_string(),
            }
        );
    }

    fn weekly_mo_we() -> TaskRecurrence {
        TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 2,
            day_of_week: Some(vec![Weekday::Monday, Weekday::Wednesday]),
            day_of_month: None,
            end: None,
            anchor: RecurrenceAnchor::FromDate,
            placement: RecurrencePlacement::Schedule,
            fixed_dates: None,
        }
    }

    #[test]
    fn recurrence_serializes_to_rrule() {
        assert_eq!(
            task_recurrence_to_rrule(weekly_mo_we()).unwrap(),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE"
        );
    }

    #[test]
    fn rrule_round_trips_through_the_boundary() {
        let parsed =
            rrule_to_task_recurrence("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE".to_string()).unwrap();
        assert_eq!(parsed, weekly_mo_we());
        assert_eq!(
            task_recurrence_to_rrule(parsed).unwrap(),
            "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE"
        );
    }

    #[test]
    fn until_date_crosses_as_iso_string() {
        let mut rec = weekly_mo_we();
        rec.end = Some(RecurrenceEnd::OnDate {
            date: "2026-12-31".to_string(),
        });
        assert!(task_recurrence_to_rrule(rec)
            .unwrap()
            .contains("UNTIL=20261231"));
    }

    #[test]
    fn invalid_until_date_surfaces_an_error() {
        let mut rec = weekly_mo_we();
        rec.end = Some(RecurrenceEnd::OnDate {
            date: "31.12.2026".to_string(),
        });
        assert!(matches!(
            task_recurrence_to_rrule(rec),
            Err(RecurrenceError::InvalidDate { .. })
        ));
    }

    #[test]
    fn rrule_without_freq_is_rejected() {
        assert!(rrule_to_task_recurrence("INTERVAL=2".to_string()).is_none());
    }

    #[test]
    fn local_store_round_trips_a_task_list() {
        // `:memory:` gives the store its own fresh, fully-migrated DB —
        // proving the open + aperio_db::run + LocalAdapter path end to end.
        let store = LocalStore::open(":memory:".to_string()).unwrap();
        let created = store.create_task_list("Groceries".to_string()).unwrap();
        assert_eq!(created.name, "Groceries");
        assert!(!created.read_only);

        // Read it back through the same store: real persistence in SQLite.
        let fetched = store.task_list(created.id.clone()).unwrap();
        assert_eq!(fetched, created);

        // A missing id surfaces NotFound, not a panic.
        assert!(matches!(
            store.task_list("does-not-exist".to_string()),
            Err(StoreError::NotFound)
        ));
    }

    /// A minimal open task with everything optional left unset.
    fn new_task_dto(title: &str) -> NewTaskDto {
        NewTaskDto {
            title: title.to_string(),
            description: None,
            status: TaskStatus::Open,
            priority: TaskPriority::Medium,
            scheduled_date: None,
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: vec![],
            sound: None,
        }
    }

    /// A fresh in-memory store with a single empty task list.
    fn store_with_list() -> (Arc<LocalStore>, TaskListDto) {
        let store = LocalStore::open(":memory:".to_string()).unwrap();
        let list = store.create_task_list("Inbox".to_string()).unwrap();
        (store, list)
    }

    #[test]
    fn local_store_round_trips_a_task() {
        let (store, list) = store_with_list();

        let mut nt = new_task_dto("Buy milk");
        nt.description = Some("2% please".to_string());
        nt.priority = TaskPriority::High;
        let created = store.create_task(list.id.clone(), nt).unwrap();
        assert_eq!(created.title, "Buy milk");
        assert_eq!(created.list_id, list.id);
        assert_eq!(created.status, TaskStatus::Open);
        assert_eq!(created.priority, TaskPriority::High);
        assert_eq!(created.description.as_deref(), Some("2% please"));

        // The read paths agree with each other and echo the stable fields.
        let listed = store.tasks(list.id.clone()).unwrap();
        assert_eq!(listed.len(), 1);
        let fetched = store.task(created.id.clone()).unwrap();
        assert_eq!(listed[0], fetched);
        assert_eq!(fetched.id, created.id);
        assert_eq!(fetched.title, created.title);
        assert_eq!(fetched.description, created.description);
        assert_eq!(fetched.priority, created.priority);

        // A missing task surfaces NotFound, not a panic.
        assert!(matches!(
            store.task("does-not-exist".to_string()),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn task_dates_and_times_cross_as_strings() {
        let (store, list) = store_with_list();
        let mut nt = new_task_dto("Standup");
        nt.scheduled_date = Some("2026-05-21".to_string());
        nt.scheduled_time = Some("09:30:00".to_string());
        nt.deadline_date = Some("2026-07-31".to_string());
        store.create_task(list.id.clone(), nt).unwrap();

        let tasks = store.tasks(list.id).unwrap();
        let t = &tasks[0];
        assert_eq!(t.scheduled_date.as_deref(), Some("2026-05-21"));
        assert_eq!(t.scheduled_time.as_deref(), Some("09:30:00"));
        assert_eq!(t.deadline_date.as_deref(), Some("2026-07-31"));
        assert_eq!(t.deadline_time, None);
    }

    #[test]
    fn reminders_and_sound_survive_the_round_trip() {
        let (store, list) = store_with_list();
        let mut nt = new_task_dto("Take meds");
        nt.sound = Some(SoundConfig {
            source: SoundSource::Silent,
            volume: 50,
        });
        nt.reminders = vec![
            Reminder {
                kind: ReminderKind::Relative { minutes_before: 30 },
                sound: None,
            },
            Reminder {
                kind: ReminderKind::Absolute {
                    at: "2026-05-19T08:00:00+00:00".to_string(),
                },
                sound: Some(SoundConfig {
                    source: SoundSource::System,
                    volume: 80,
                }),
            },
        ];
        let created = store.create_task(list.id, nt).unwrap();

        // Read back through SQLite (reminders + sound ride a JSON column).
        let fetched = store.task(created.id).unwrap();
        assert_eq!(
            fetched.sound,
            Some(SoundConfig {
                source: SoundSource::Silent,
                volume: 50,
            })
        );
        assert_eq!(fetched.reminders.len(), 2);
        assert_eq!(
            fetched.reminders[0].kind,
            ReminderKind::Relative { minutes_before: 30 }
        );
        assert_eq!(
            fetched.reminders[1].kind,
            ReminderKind::Absolute {
                at: "2026-05-19T08:00:00+00:00".to_string()
            }
        );
        assert_eq!(
            fetched.reminders[1].sound,
            Some(SoundConfig {
                source: SoundSource::System,
                volume: 80,
            })
        );
    }

    #[test]
    fn tasks_can_be_updated_and_deleted() {
        let (store, list) = store_with_list();
        let created = store
            .create_task(list.id.clone(), new_task_dto("Draft"))
            .unwrap();

        // A full-overwrite update edits the title; the rest is preserved.
        let mut edit = created.clone();
        edit.title = "Final".to_string();
        let updated = store.update_task(edit).unwrap();
        assert_eq!(updated.title, "Final");
        assert_eq!(store.task(created.id.clone()).unwrap().title, "Final");

        // Delete removes it; a second delete is NotFound.
        store.delete_task(created.id.clone()).unwrap();
        assert!(store.tasks(list.id).unwrap().is_empty());
        assert!(matches!(
            store.delete_task(created.id).unwrap_err(),
            StoreError::NotFound
        ));
    }

    #[test]
    fn completing_a_recurring_task_spawns_the_next_through_ffi() {
        let (store, list) = store_with_list();
        let mut nt = new_task_dto("Water plants");
        nt.scheduled_date = Some("2026-05-19".to_string());
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 3,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor: RecurrenceAnchor::FromDate,
            placement: RecurrencePlacement::Schedule,
            fixed_dates: None,
        });
        let created = store.create_task(list.id.clone(), nt).unwrap();
        assert!(
            created.series_id.is_some(),
            "a recurring task gets a stable series id"
        );

        // Completing it spawns the next instance (DESIGN §9.12).
        let mut done = created.clone();
        done.status = TaskStatus::Completed;
        done.completed_at = Some("2026-05-19T09:00:00+00:00".to_string());
        store.update_task(done).unwrap();

        let tasks = store.tasks(list.id).unwrap();
        assert_eq!(tasks.len(), 2, "the completed task plus a fresh instance");
        let next = tasks
            .iter()
            .find(|t| t.status == TaskStatus::Open)
            .expect("a fresh open instance was spawned");
        assert_eq!(next.scheduled_date.as_deref(), Some("2026-05-22"));
    }

    #[test]
    fn invalid_date_field_surfaces_invalid_field() {
        let (store, list) = store_with_list();
        let mut nt = new_task_dto("Bad date");
        nt.scheduled_date = Some("21.05.2026".to_string()); // not YYYY-MM-DD
        assert!(matches!(
            store.create_task(list.id, nt),
            Err(StoreError::InvalidField { .. })
        ));
    }

    #[test]
    fn task_lists_can_be_listed_renamed_and_deleted() {
        let store = LocalStore::open(":memory:".to_string()).unwrap();
        let beta = store.create_task_list("Beta".to_string()).unwrap();
        let alpha = store.create_task_list("Alpha".to_string()).unwrap();

        // Listed sorted by name, case-insensitive: Alpha before Beta.
        let names: Vec<String> = store
            .task_lists()
            .unwrap()
            .into_iter()
            .map(|l| l.name)
            .collect();
        assert_eq!(names, vec!["Alpha".to_string(), "Beta".to_string()]);

        // Rename, and reject an all-whitespace name.
        store
            .rename_task_list(alpha.id.clone(), "Gamma".to_string())
            .unwrap();
        assert_eq!(store.task_list(alpha.id.clone()).unwrap().name, "Gamma");
        assert!(matches!(
            store.rename_task_list(alpha.id, "   ".to_string()),
            Err(StoreError::InvalidField { .. })
        ));

        // Delete, then a missing-list delete is NotFound.
        store.delete_task_list(beta.id.clone()).unwrap();
        assert!(matches!(
            store.task_list(beta.id).unwrap_err(),
            StoreError::NotFound
        ));
        assert_eq!(store.task_lists().unwrap().len(), 1);
        assert!(matches!(
            store.delete_task_list("does-not-exist".to_string()),
            Err(StoreError::NotFound)
        ));
    }

    #[test]
    fn updating_a_missing_task_is_not_found() {
        let (store, list) = store_with_list();
        let created = store.create_task(list.id, new_task_dto("Ghost")).unwrap();
        store.delete_task(created.id.clone()).unwrap();
        // The DTO is well-formed, but the row is gone.
        assert!(matches!(
            store.update_task(created).unwrap_err(),
            StoreError::NotFound
        ));
    }

    #[test]
    fn update_preserves_store_managed_series_id_and_resurface_date() {
        let (store, list) = store_with_list();
        // A backlog rule: completing it spawns an undated instance whose
        // resurface_date is computed from the completion (DESIGN §9.12).
        let mut nt = new_task_dto("Water the plant");
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: None,
            anchor: RecurrenceAnchor::FromCompletion,
            placement: RecurrencePlacement::Backlog,
            fixed_dates: None,
        });
        let created = store.create_task(list.id.clone(), nt).unwrap();
        assert!(
            created.series_id.is_some(),
            "a recurring task is assigned a series id on create"
        );

        // Complete on a known day → spawns the next backlog instance.
        let mut done = created.clone();
        done.status = TaskStatus::Completed;
        done.completed_at = Some("2026-05-10T09:00:00+00:00".to_string());
        store.update_task(done).unwrap();

        let spawned = store
            .tasks(list.id.clone())
            .unwrap()
            .into_iter()
            .find(|t| t.status == TaskStatus::Open)
            .expect("a fresh backlog instance was spawned");
        // Completed 10 May + 1 week → resurfaces 17 May; same series.
        assert_eq!(spawned.resurface_date.as_deref(), Some("2026-05-17"));
        let series = spawned.series_id.clone();
        assert!(series.is_some());

        // Editing an ordinary field must not disturb the store-managed
        // series_id / resurface_date — they survive the update round-trip.
        let mut edit = spawned.clone();
        edit.title = "Water the fern".to_string();
        store.update_task(edit).unwrap();

        let reread = store.task(spawned.id).unwrap();
        assert_eq!(reread.title, "Water the fern");
        assert_eq!(reread.series_id, series);
        assert_eq!(reread.resurface_date.as_deref(), Some("2026-05-17"));
    }

    #[test]
    fn json_bridge_round_trips_a_task() {
        let (store, list) = store_with_list();
        // A minimal cal_core::NewTask in the serde shape the desktop also uses.
        let new_json = r#"{"title":"From JSON","description":null,"status":"open","priority":"medium","scheduled_date":"2026-05-21","scheduled_time":null,"deadline_date":null,"deadline_time":null,"recurrence":null,"parent_id":null,"color_label":null,"reminders":[],"sound":null}"#;
        let created = store
            .create_task_json(list.id.clone(), new_json.to_string())
            .unwrap();
        assert!(created.contains("\"title\":\"From JSON\""));
        assert!(created.contains("\"scheduled_date\":\"2026-05-21\""));

        let listed = store.tasks_json(list.id.clone()).unwrap();
        assert!(listed.contains("From JSON"));

        // The full task round-trips back through update unchanged.
        let updated = store.update_task_json(created).unwrap();
        assert!(updated.contains("From JSON"));

        // Malformed JSON surfaces a typed InvalidField, not a panic.
        assert!(matches!(
            store.create_task_json(list.id, "{not json}".to_string()),
            Err(StoreError::InvalidField { .. })
        ));
    }

    #[test]
    fn json_bridge_round_trips_lists_and_sections() {
        let store = LocalStore::open(":memory:".to_string()).unwrap();
        let list_json = store.create_task_list_json("Project".to_string()).unwrap();
        assert!(list_json.contains("\"name\":\"Project\""));
        let list_id = store.task_lists().unwrap()[0].id.clone();

        let section = store
            .create_section_json(list_id.clone(), "Doing".to_string(), 0, None)
            .unwrap();
        assert!(section.contains("\"name\":\"Doing\""));
        let sections = store.sections_json(list_id).unwrap();
        assert!(sections.contains("Doing"));
    }
}
