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

/// The ISO date format used for `RecurrenceEnd::OnDate` across the boundary.
const DATE_FMT: &str = "%Y-%m-%d";

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
}
