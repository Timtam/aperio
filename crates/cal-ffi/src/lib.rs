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

/// The task view's rows: which group each task lands in, in which order,
/// under which header, at what depth. Asked while rendering, so synchronous.
#[uniffi::export]
pub fn group_tasks(input_json: String) -> Result<String, StoreError> {
    cal_core::group_tasks_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "grouping input".into(),
        detail: e.to_string(),
    })
}

/// The tasks a calendar day shows, with what each chip carries. Asked inside `useMemo`, so synchronous.
#[uniffi::export]
pub fn tasks_on_days(input_json: String) -> Result<String, StoreError> {
    cal_core::tasks_on_days_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "task day input".into(),
        detail: e.to_string(),
    })
}
/// The two calendar weeks the backlog rail splits its deadlines into.
#[uniffi::export]
pub fn backlog_weeks(input_json: String) -> Result<String, StoreError> {
    cal_core::backlog_weeks_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "task day input".into(),
        detail: e.to_string(),
    })
}
/// Deadline-carrying items by week, as positions.
#[uniffi::export]
pub fn split_deadlines_by_week(input_json: String) -> Result<String, StoreError> {
    cal_core::split_deadlines_by_week_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "task day input".into(),
        detail: e.to_string(),
    })
}

/// Every i18n key of the task vocabulary, as one table: read once at
/// install, never asked per chip.
#[uniffi::export]
pub fn task_i18n_keys() -> String {
    cal_core::task_i18n_keys_json()
}

/// How far every parent's subtasks are, for a whole task list at once.
#[uniffi::export]
pub fn subtask_progress(input_json: String) -> Result<String, StoreError> {
    cal_core::subtask_progress_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "subtask progress input".into(),
        detail: e.to_string(),
    })
}

/// The writes a task's status change plans: the root, its descendants, its
/// ancestors — in application order.
#[uniffi::export]
pub fn plan_status_cascade(input_json: String) -> Result<String, StoreError> {
    cal_core::plan_status_cascade_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "status cascade input".into(),
        detail: e.to_string(),
    })
}

/// The ancestors re-derived after a subtask was created or deleted.
#[uniffi::export]
pub fn plan_ancestor_recompute(input_json: String) -> Result<String, StoreError> {
    cal_core::plan_ancestor_recompute_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "ancestor recompute input".into(),
        detail: e.to_string(),
    })
}

/// The "started → pin to today" companion date, or null.
#[uniffi::export]
pub fn auto_date_on_start(input_json: String) -> Result<String, StoreError> {
    cal_core::auto_date_on_start_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "auto-date input".into(),
        detail: e.to_string(),
    })
}

/// The occurrences of recurring scheduled tasks inside a window: which
/// input task, on which day, real or projected.
#[uniffi::export]
pub fn expand_task_occurrences(input_json: String) -> Result<String, StoreError> {
    cal_core::expand_task_occurrences_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "task occurrences input".into(),
        detail: e.to_string(),
    })
}

/// The next occurrence of a repeating task after a day, or null.
#[uniffi::export]
pub fn next_task_occurrence(input_json: String) -> Result<String, StoreError> {
    cal_core::next_task_occurrence_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "next occurrence input".into(),
        detail: e.to_string(),
    })
}

/// What "move to this day" can be on a source that owns the date.
#[uniffi::export]
pub fn occurrence_move_target(input_json: String) -> Result<String, StoreError> {
    cal_core::occurrence_move_target_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "occurrence move input".into(),
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
}
