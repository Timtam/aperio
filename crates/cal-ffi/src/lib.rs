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

/// The zone a series repeats on: the stored name when it is a zone, "" when the
/// series repeats on UTC. See cal_core::series_clock_zone.
#[uniffi::export]
pub fn series_clock_zone(tzid: String) -> String {
    cal_core::series_clock_zone(Some(&tzid))
        .unwrap_or("")
        .to_string()
}

/// tzdata's spelling of the zone a name resolves to; "" for a name tzdata does
/// not know. See cal_core::canonical_zone.
#[uniffi::export]
pub fn canonical_zone(name: String) -> String {
    cal_core::canonical_zone(&name).unwrap_or("").to_string()
}

/// The world zone list's names by position, as JSON. See cal_core::zone_list.
#[uniffi::export]
pub fn zone_labels() -> Result<String, StoreError> {
    cal_core::zone_labels_json().map_err(|e| StoreError::InvalidField {
        field: "zone labels".into(),
        detail: e.to_string(),
    })
}

/// A search over the world zone list, as JSON. See cal_core::zone_search.
#[uniffi::export]
pub fn zone_search(input_json: String) -> Result<String, StoreError> {
    cal_core::zone_search_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "zone search question".into(),
        detail: e.to_string(),
    })
}

/// Where a stored zone and the device's zone stand in the list, as JSON. See
/// cal_core::zone_choice.
#[uniffi::export]
pub fn zone_choice(input_json: String) -> Result<String, StoreError> {
    cal_core::zone_choice_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "zone choice question".into(),
        detail: e.to_string(),
    })
}

/// Every listed zone's offsets and the order of the list, as JSON. See
/// cal_core::zone_offsets.
#[uniffi::export]
pub fn zone_offsets(input_json: String) -> Result<String, StoreError> {
    cal_core::zone_offsets_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "zone offsets question".into(),
        detail: e.to_string(),
    })
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

/// Who holds a task after a status change: positions of the assignees that
/// stay, or "take me", or nothing.
#[uniffi::export]
pub fn self_assign_on_status_change(input_json: String) -> Result<String, StoreError> {
    cal_core::self_assign_on_status_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "self-assign input".into(),
        detail: e.to_string(),
    })
}

/// How many people a list can hold on one task.
#[uniffi::export]
pub fn task_assignment_mode(input_json: String) -> Result<String, StoreError> {
    cal_core::task_assignment_mode_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "assignment mode input".into(),
        detail: e.to_string(),
    })
}

/// Which of the given assignees a list can hold: positions.
#[uniffi::export]
pub fn clamp_assignees(input_json: String) -> Result<String, StoreError> {
    cal_core::clamp_assignees_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "clamp assignees input".into(),
        detail: e.to_string(),
    })
}

/// What a description's signature block says, or null.
#[uniffi::export]
pub fn signature_in(input_json: String) -> Result<String, StoreError> {
    cal_core::signature_in_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "signature input".into(),
        detail: e.to_string(),
    })
}

/// The description without its signature block.
#[uniffi::export]
pub fn strip_signature(input_json: String) -> Result<String, StoreError> {
    cal_core::strip_signature_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "signature input".into(),
        detail: e.to_string(),
    })
}

/// The description with a body as its signature block — replacing, never
/// stacking.
#[uniffi::export]
pub fn apply_signature(input_json: String) -> Result<String, StoreError> {
    cal_core::apply_signature_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "apply signature input".into(),
        detail: e.to_string(),
    })
}

/// One question to the day-start rules, and its answer.
#[uniffi::export]
pub fn day_start(input_json: String) -> Result<String, StoreError> {
    cal_core::day_start_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "day start question".into(),
        detail: e.to_string(),
    })
}

/// One question to the task-settings rules, and its answer.
#[uniffi::export]
pub fn task_settings(input_json: String) -> Result<String, StoreError> {
    cal_core::task_settings_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "task settings question".into(),
        detail: e.to_string(),
    })
}

/// Shifting a recurring series by whole days: the rule a whole-series move
/// writes, or why it cannot move. The desktop reaches the same rule through
/// WebAssembly; the series time zone choice needs it on the phone too.
#[uniffi::export]
pub fn series_shift(input_json: String) -> Result<String, StoreError> {
    cal_core::series_shift_json(&input_json).map_err(|e| StoreError::InvalidField {
        field: "series shift question".into(),
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

    #[test]
    fn series_shift_door_answers_like_the_core() {
        assert_eq!(
            series_shift(
                r#"{"rrule":"FREQ=WEEKLY;BYDAY=MO","start":"2026-05-04","days":1}"#.to_string()
            )
            .expect("a readable question"),
            r#"{"outcome":"shifted","rrule":"FREQ=WEEKLY;BYDAY=TU"}"#
        );
        assert_eq!(
            series_shift(
                r#"{"rrule":"FREQ=MONTHLY;BYDAY=2SU","start":"2026-05-10","days":1}"#.to_string()
            )
            .expect("a readable question"),
            r#"{"outcome":"refused","reason":"ordinal_weekday"}"#
        );
    }

    #[test]
    fn series_shift_door_names_a_question_it_cannot_read() {
        match series_shift("not json".to_string()) {
            Err(StoreError::InvalidField { field, .. }) => {
                assert_eq!(field, "series shift question")
            }
            other => panic!("expected an invalid-field error, got {other:?}"),
        }
    }

    /// Every row of the core's series-clock table, through the phone's doors,
    /// which write "none" as "" and could get that wrong where the core cannot.
    #[test]
    fn series_clock_doors_answer_every_contract_row() {
        const CONTRACT: &str = include_str!("../../cal-core/tests/fixtures/seriesClock.json");
        let doc: serde_json::Value = serde_json::from_str(CONTRACT).expect("the contract parses");

        let rows = doc["seriesClockZone"]
            .as_array()
            .expect("seriesClockZone rows");
        // The rows that reach the door's "" encoding, and the rows the rule turns on.
        assert!(
            rows.iter().any(|r| r["tzid"].is_null()),
            "the contract lost the row without a zone",
        );
        for tzid in ["", "Etc/UTC", "europe/berlin", "+05:30"] {
            assert!(
                rows.iter().any(|r| r["tzid"].as_str() == Some(tzid)),
                "the contract lost the {tzid:?} row",
            );
        }
        for row in rows {
            let tzid = row["tzid"].as_str().unwrap_or("").to_string();
            let want = row["zone"].as_str().unwrap_or("");
            assert_eq!(series_clock_zone(tzid.clone()), want, "{tzid:?}");
        }

        let canonical = doc["canonicalZone"].as_array().expect("canonicalZone rows");
        for name in ["", "asia/calcutta", "Europe/Oslo", "UTC"] {
            assert!(
                canonical.iter().any(|r| r["name"].as_str() == Some(name)),
                "the contract lost the {name:?} canonical row",
            );
        }
        for row in canonical {
            let name = row["name"]
                .as_str()
                .expect("every row names a zone")
                .to_string();
            let want = row["canonical"].as_str().unwrap_or("");
            assert_eq!(canonical_zone(name.clone()), want, "{name:?}");
        }
    }

    /// The core's zone list table through the phone's doors, every section but
    /// the fold, which no door hands out: the JSON each door reads and writes,
    /// which the core's own test never crosses.
    #[test]
    fn zone_list_doors_answer_every_contract_row() {
        use serde_json::{json, Value};

        const CONTRACT: &str = include_str!("../../cal-core/tests/fixtures/timeZoneFilter.json");
        let doc: Value = serde_json::from_str(CONTRACT).expect("the contract parses");
        let labels: Vec<Value> =
            serde_json::from_str(&zone_labels().expect("the labels")).expect("a list");
        let zone_of = |entry: &Value| -> String {
            match entry["position"].as_u64() {
                Some(position) => labels[position as usize]["zone"]
                    .as_str()
                    .expect("a zone")
                    .to_string(),
                None => "utc".to_string(),
            }
        };

        // Anti-silence, here and below: the rows the rules turn on, by name, the
        // same the core names.
        let label_rows = doc["labels"].as_array().expect("label rows");
        for zone in ["Europe/Berlin", "America/Indiana/Indianapolis"] {
            assert!(
                label_rows.iter().any(|row| row["zone"] == zone),
                "the contract lost the {zone} label row"
            );
        }
        for row in label_rows {
            assert!(labels.contains(row), "{row}");
        }
        let not_listed = doc["notListed"].as_array().expect("notListed");
        for name in ["EST5EDT", "Europe/Oslo"] {
            assert!(
                not_listed.iter().any(|listed| listed == name),
                "the contract lost {name} from notListed"
            );
        }
        for name in not_listed {
            assert!(
                labels.iter().all(|label| label["zone"] != *name),
                "{name} is listed"
            );
        }

        let rows = doc["search"].as_array().expect("search rows");
        // Anti-silence: the same rows the core names, since this door runs
        // every one of them, offsets included.
        for query in [
            "kiev",
            "Oslo",
            "Montreal",
            "Zuerich",
            "Wien",
            "Europa",
            "GMT",
            "   ",
            "5:30",
            "−3:30",
            "+5",
            "berlin europa",
            "enix",
            "Europe/Kiev",
            "+0",
            "+00:09",
        ] {
            assert!(
                rows.iter().any(|row| row["query"] == query),
                "the contract lost the {query:?} row"
            );
        }
        for row in rows {
            let query = row["query"].as_str().expect("a query");
            let regions: Vec<Value> = doc["regions"][row["regions"].as_str().expect("a set")]
                .as_object()
                .expect("region names")
                .iter()
                .map(|(region, name)| json!({ "region": region, "name": name }))
                .collect();
            let offsets = match row.get("offsets") {
                Some(asked) => serde_json::from_str::<Value>(
                    &zone_offsets(asked.to_string()).expect("the offsets"),
                )
                .expect("offsets JSON"),
                None => Value::Null,
            };
            let question = json!({ "query": query, "region_names": regions, "offsets": offsets });
            let answer: Value =
                serde_json::from_str(&zone_search(question.to_string()).expect("a search"))
                    .expect("an answer");
            let hits = answer["hits"].as_array().expect("hits");
            let hit_is = |hit: &Value, want: &Value| {
                zone_of(&hit["entry"]) == want["zone"].as_str().unwrap_or_default()
                    && hit["via"] == want["via"]
                    && match want.get("also") {
                        None => hit["also"].is_null(),
                        Some(also) => {
                            hit["also"]["label"] == also["label"]
                                && also
                                    .get("name")
                                    .is_none_or(|name| hit["also"]["name"] == *name)
                                && also
                                    .get("kind")
                                    .is_none_or(|kind| hit["also"]["kind"] == *kind)
                        }
                    }
            };
            if let Some(want) = row["hits"].as_array() {
                assert_eq!(hits.len(), want.len(), "{query:?}");
                for (hit, want) in hits.iter().zip(want) {
                    assert!(hit_is(hit, want), "{query:?}: got {hit}, want {want}");
                }
            }
            if let Some(count) = row["count"].as_u64() {
                assert_eq!(hits.len() as u64, count, "{query:?}");
            }
            for want in row["includes"].as_array().into_iter().flatten() {
                assert!(
                    hits.iter().any(|hit| hit_is(hit, want)),
                    "{query:?}: missing {want}"
                );
            }
            for zone in row["excludes"].as_array().into_iter().flatten() {
                let zone = zone.as_str().unwrap_or_default();
                assert!(
                    hits.iter().all(|hit| zone_of(&hit["entry"]) != zone),
                    "{query:?}: {zone} must not be a hit"
                );
            }
        }

        // A question the core refuses comes back as an error, not a panic.
        let short: Vec<Value> = doc["regions"]["de"]
            .as_object()
            .expect("region names")
            .iter()
            .skip(1)
            .map(|(region, name)| json!({ "region": region, "name": name }))
            .collect();
        let refused = zone_search(json!({ "query": "berlin", "region_names": short }).to_string());
        assert!(
            matches!(refused, Err(StoreError::InvalidField { .. })),
            "{refused:?}"
        );

        let plain = |choice: &Value| -> Value {
            let mut choice = choice.clone();
            if let Some(object) = choice.as_object_mut() {
                if let Some(position) = object.remove("position") {
                    let position = position.as_u64().expect("a position") as usize;
                    object.insert("zone".to_string(), labels[position]["zone"].clone());
                }
            }
            choice
        };
        let choices = doc["choice"].as_array().expect("choice rows");
        assert!(
            choices.iter().any(|row| row["stored"] == "Europe/Kiev"),
            "the contract lost the stored Europe/Kiev row"
        );
        assert!(
            choices.iter().any(|row| row["device"] == "Asia/Calcutta"),
            "the contract lost the device Asia/Calcutta row"
        );
        for row in choices {
            let question = json!({
                "stored": row["stored"],
                "device": row.get("device").cloned().unwrap_or(Value::Null),
            });
            let answer: Value =
                serde_json::from_str(&zone_choice(question.to_string()).expect("a choice"))
                    .expect("an answer");
            assert_eq!(
                plain(&answer["stored"]),
                row["expect"]["stored"],
                "{question}"
            );
            assert_eq!(
                plain(&answer["device"]),
                row["expect"].get("device").cloned().unwrap_or(Value::Null),
                "{question}"
            );
        }

        let offset_rows = doc["offsets"].as_array().expect("offset rows");
        for zone in [
            "Europe/Dublin",
            "Australia/Lord_Howe",
            "Europe/Paris",
            "Asia/Almaty",
        ] {
            assert!(
                offset_rows.iter().any(|row| row["zone"] == zone),
                "the contract lost the {zone} rows"
            );
        }
        for row in offset_rows {
            let asked = json!({ "at": row["at"], "today": row["today"] });
            let answer: Value =
                serde_json::from_str(&zone_offsets(asked.to_string()).expect("the offsets"))
                    .expect("offsets JSON");
            let position = labels
                .iter()
                .position(|label| label["zone"] == row["zone"])
                .expect("a listed zone");
            let listed = &answer["zones"][position];
            assert_eq!(listed["offset"]["seconds"], row["seconds"], "{row}");
            for field in ["sign", "hh", "mm"] {
                if let Some(want) = row.get(field) {
                    assert_eq!(listed["offset"][field], *want, "{field} of {row}");
                }
            }
            if let Some(standard) = row.get("standard") {
                assert_eq!(
                    listed["standard"]["seconds"], *standard,
                    "standard of {row}"
                );
            }
        }

        let order_rows = doc["order"].as_array().expect("order rows");
        assert!(
            order_rows
                .iter()
                .any(|row| row["utcBefore"] == "Africa/Abidjan"),
            "the contract lost the order row"
        );
        for row in order_rows {
            let asked = json!({ "at": row["at"], "today": row["today"] });
            let answer: Value =
                serde_json::from_str(&zone_offsets(asked.to_string()).expect("the offsets"))
                    .expect("offsets JSON");
            let names: Vec<String> = answer["order"]
                .as_array()
                .expect("the order")
                .iter()
                .map(&zone_of)
                .collect();
            let zones = |key: &str| -> Vec<String> {
                row[key]
                    .as_array()
                    .expect("zones")
                    .iter()
                    .map(|zone| zone.as_str().expect("a zone").to_string())
                    .collect()
            };
            let head = zones("head");
            let tail = zones("tail");
            assert_eq!(names[..head.len()], head[..], "the head of the list");
            assert_eq!(
                names[names.len() - tail.len()..],
                tail[..],
                "the tail of the list"
            );
            let utc = names
                .iter()
                .position(|name| name == "utc")
                .expect("the UTC entry");
            assert_eq!(
                names[utc + 1],
                row["utcBefore"].as_str().unwrap_or_default()
            );
        }
    }
}
