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

/// Process-global rolling-file log sink + level control (the mobile twin of the
/// desktop's Tauri-managed `LogState`). Installed once from `Host::open`.
mod logging;

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

// ───────────────────────────── Text ordering ────────────────────────────────
//
// The two rules every list on this app ends up in for its tiebreaker, from
// `cal_core::collation`. They cross as SYNCHRONOUS bridge functions (Expo
// `Function`, not `AsyncFunction`) because their callers are `Array.prototype
// .sort` comparators, which cannot await — the same reason the desktop reaches
// them through WebAssembly. See DESIGN §4.4.
//
// They answer -1/0/1 rather than an enum: a comparator is what consumes them,
// and every extra shape on this boundary is one more thing the Swift and
// Kotlin halves can spell differently.

/// Compare two NAMES — an account, a container, a contact, a day marker.
/// Case- and accent-insensitive. See `cal_core::compare_names`.
#[uniffi::export]
pub fn compare_names(a: String, b: String, language_tag: String) -> i32 {
    ordering_to_i32(cal_core::compare_names(
        &a,
        &b,
        cal_core::CollationLanguage::from_tag(&language_tag),
    ))
}

/// Compare two TITLES — task titles, section names, anything the user typed.
/// Digit runs order by value; case separates. See `cal_core::compare_titles`.
#[uniffi::export]
pub fn compare_titles(a: String, b: String, language_tag: String) -> i32 {
    ordering_to_i32(cal_core::compare_titles(
        &a,
        &b,
        cal_core::CollationLanguage::from_tag(&language_tag),
    ))
}

fn ordering_to_i32(ordering: std::cmp::Ordering) -> i32 {
    match ordering {
        std::cmp::Ordering::Less => -1,
        std::cmp::Ordering::Equal => 0,
        std::cmp::Ordering::Greater => 1,
    }
}

// ──────────────────────────── Task priority ─────────────────────────────────
//
// The ranking every task list sorts by, from `cal_core::task_priority`. It
// crosses SYNCHRONOUSLY (Expo `Function`, not `AsyncFunction`) because its
// callers are `Array.prototype.sort` comparators, which cannot await — the
// same reason the desktop reaches it through WebAssembly.
//
// Until this existed the rule was reachable from the desktop ONLY: it lived in
// `crates/cal-core-wasm`, the desktop's own binding, so mobile went on running
// the TypeScript copy. Two devices showing one task list have to put the same
// task first.
//
// The scale and the priority cross as the lowercase strings serde already
// writes for them everywhere else in this app. An unknown value is an ERROR
// rather than a default: answering confidently for a value nobody wrote is how
// a list quietly sorts wrong.

/// The sort rank of a priority under a scale; 0 sorts first.
///
/// `scale` is `"three"` or `"two"`; `priority` is `"low"`, `"medium"` or
/// `"high"`. See `cal_core::priority_rank`.
#[uniffi::export]
pub fn priority_rank(priority: String, scale: String) -> Result<u32, StoreError> {
    Ok(cal_core::priority_rank(
        parse_priority(&priority)?,
        parse_scale(&scale)?,
    ))
}

/// Whether a priority is the TOP one — "important" in the two-level system.
/// See `cal_core::TaskPriority::is_important`.
#[uniffi::export]
pub fn is_important_priority(priority: String) -> Result<bool, StoreError> {
    Ok(parse_priority(&priority)?.is_important())
}

/// The priority a task gets when "important" is cleared: what it already had,
/// unless that was the top one. An empty string means "nothing before".
/// See `cal_core::normal_priority`.
#[uniffi::export]
pub fn normal_priority(previous: String) -> Result<String, StoreError> {
    let kept = if previous.is_empty() {
        None
    } else {
        Some(parse_priority(&previous)?)
    };
    Ok(priority_to_wire(cal_core::normal_priority(kept)).to_string())
}

fn parse_priority(value: &str) -> Result<cal_core::TaskPriority, StoreError> {
    match value {
        "low" => Ok(cal_core::TaskPriority::Low),
        "medium" => Ok(cal_core::TaskPriority::Medium),
        "high" => Ok(cal_core::TaskPriority::High),
        other => Err(StoreError::InvalidField {
            field: "priority".into(),
            detail: format!("unknown priority {other:?}; expected low, medium or high"),
        }),
    }
}

fn parse_scale(value: &str) -> Result<cal_core::PriorityScale, StoreError> {
    match value {
        "two" => Ok(cal_core::PriorityScale::Two),
        "three" => Ok(cal_core::PriorityScale::Three),
        other => Err(StoreError::InvalidField {
            field: "scale".into(),
            detail: format!("unknown priority scale {other:?}; expected two or three"),
        }),
    }
}

fn priority_to_wire(priority: cal_core::TaskPriority) -> &'static str {
    match priority {
        cal_core::TaskPriority::Low => "low",
        cal_core::TaskPriority::Medium => "medium",
        cal_core::TaskPriority::High => "high",
    }
}

// ────────────────────────── Conference detection ────────────────────────────
//
// Finding the online meeting in an event, from `cal_core::conferencing`. It
// crosses as JSON in both directions — the same shape the desktop's WebAssembly
// door uses, and the same shape the task domain already crosses here.
//
// The marshalling is `detect_conference_json` in the core, not a copy on each
// side. Two copies of a marshalling step is precisely how the detection RULE
// came to exist twice — once here and once in `shared/conferencing.ts` — with
// nothing pinning them together and six places where they had drifted apart.

/// Find the online meeting in an event, or answer `"null"`.
///
/// `sources_json` is `{providerField?, icalendarConference[],
/// vendorProperties[], location?, description?}`. The answer is a
/// `ConferenceLink` as JSON, or `"null"` when there is none.
#[uniffi::export]
pub fn detect_conference(sources_json: String) -> Result<String, StoreError> {
    cal_core::conferencing::detect_conference_json(&sources_json).map_err(|e| {
        StoreError::InvalidField {
            field: "conference sources".into(),
            detail: e.to_string(),
        }
    })
}

// ────────────────────────── Recognising a copy ──────────────────────────────
//
// Same arrangement, same reason: the marshalling is in the core and both doors
// call it. These two answer with POSITIONS in the input rather than the rows
// themselves — the caller is holding the events already, and echoing them back
// would double the payload to say nothing new.

/// Copies worth offering among one day's rows, as JSON.
///
/// `input_json` is `{events[], groups[], declines[]}`, where each event is
/// `{calendar_id, series_id, title, start, all_day}` — the same casing the
/// group and decline rows already travel in. The answer is a
/// `[{first, second}]` array of positions in `events`.
#[uniffi::export]
pub fn find_group_suggestions(input_json: String) -> Result<String, StoreError> {
    cal_core::find_group_suggestions_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "group suggestion input".into(),
        detail: e.to_string(),
    })
}

/// The row that most looks like a copy of an anchor, as JSON.
///
/// `input_json` is `{anchor, candidates[]}`. The answer is the position in
/// `candidates`, or `"null"` when nothing there is a copy.
#[uniffi::export]
pub fn suggest_group_mate(input_json: String) -> Result<String, StoreError> {
    cal_core::suggest_group_mate_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "group mate input".into(),
        detail: e.to_string(),
    })
}

/// Which rows of a window survive the meeting-duplicate filter, as JSON.
///
/// `events_json` is a `[{calendar_id, location?, description?, grouped}]`
/// array; the answer is the positions that stay. The whole window crosses at
/// once — the rule this replaces asked per row.
/// The (meeting, appointment) pairs that should become groups, as JSON.
///
/// `input_json` is `{events[], groups[], declines[]}`; the answer is
/// `[{meeting, event, join_url}]` with POSITIONS in `events`.
/// Fold a join URL to what two spellings of the same link agree on.
#[uniffi::export]
pub fn normalize_join_url(url: String) -> String {
    cal_core::normalize_join_url(&url)
}

/// Fold each group's members into a single row, as JSON.
///
/// `input_json` is `{events[], groups[]}`; the answer is one row per surviving
/// slot, each naming the POSITION of the event to draw.
/// What carrying an edit to a group's other copies would do, as JSON.
#[uniffi::export]
pub fn plan_carry(input_json: String) -> Result<String, StoreError> {
    cal_core::plan_carry_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "carry plan input".into(),
        detail: e.to_string(),
    })
}

/// The fields of the standalone row a carried OCCURRENCE edit creates, or
/// `"null"` when the instant cannot be read.
#[uniffi::export]
pub fn occurrence_carry_fields(input_json: String) -> Result<String, StoreError> {
    cal_core::occurrence_carry_fields_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "carry row input".into(),
        detail: e.to_string(),
    })
}

/// The fields of the row a carried "this and all following" edit creates, or
/// `"null"` when the cut point cannot be read.
#[uniffi::export]
pub fn future_carry_fields(input_json: String) -> Result<String, StoreError> {
    cal_core::future_carry_fields_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "carry row input".into(),
        detail: e.to_string(),
    })
}

/// The carried fields laid over a member's own current values.
#[uniffi::export]
pub fn carry_onto_fields(input_json: String) -> Result<String, StoreError> {
    cal_core::carry_onto_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "carry row input".into(),
        detail: e.to_string(),
    })
}

#[uniffi::export]
pub fn collapse_event_groups(input_json: String) -> Result<String, StoreError> {
    cal_core::collapse_event_groups_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "fold input".into(),
        detail: e.to_string(),
    })
}

#[uniffi::export]
pub fn find_meeting_link_pairs(input_json: String) -> Result<String, StoreError> {
    cal_core::find_meeting_link_pairs_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "meeting link input".into(),
        detail: e.to_string(),
    })
}

#[uniffi::export]
pub fn without_duplicate_meetings(events_json: String) -> Result<String, StoreError> {
    cal_core::without_duplicate_meetings_json(&events_json).map_err(|e| StoreError::InvalidField {
        field: "meeting filter input".into(),
        detail: e.to_string(),
    })
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
    /// A synchronisation failure, carrying the engine's own stable code.
    ///
    /// Everything sync used to arrive as `Storage`, so the phone showed the
    /// engine's English `Display` text while the desktop, branching on the same
    /// code, showed a translated sentence. The code travels now, and the mobile
    /// side maps it the way the desktop's `useSyncErrorMessage` does.
    ///
    /// `code` is one of `SyncError::code()`'s values: `io`, `network`, `auth`,
    /// `protocol`, `encryption_required`, `decryption_failed`, `not_found`,
    /// `schema_too_old`, `stale_device`, `internal`.
    #[error("{detail}")]
    Sync { code: String, detail: String },
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

// ── JSON boundary ───────────────────────────────────────────────────────────
//
// Values cross as the `cal_core` serde shape — the same payloads the desktop
// hands its UI, so the two backends never drift apart.

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
