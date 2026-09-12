//! A synchronous door from the desktop frontend into `cal-core`.
//!
//! # The problem this exists to answer
//!
//! Mobile can already call Rust synchronously: `Function("parseAttendee")` in
//! the Expo module (as opposed to `AsyncFunction`) is a direct call that
//! returns a value, not a promise, and it ships today. A future native
//! frontend — the reMarkable port — gets the same for free, because native code
//! links Rust and calls it.
//!
//! The desktop is the one surface that cannot. Its UI runs in a webview, and
//! the only road from a webview to the Tauri process is `invoke`, which is IPC
//! and therefore always asynchronous. That matters because a React render
//! function must return finished UI in one turn: `await` inside it is not slow,
//! it is impossible. A rule needed DURING render — and `priorityRank` is needed
//! inside `Array.prototype.sort` comparators, which cannot await at all — can
//! only move into the core if the core can be called synchronously.
//!
//! WebAssembly is that road: the same Rust, compiled into the webview instead
//! of living in another process. Instantiating the module is asynchronous, but
//! that happens ONCE at app start; afterwards every call is an ordinary
//! synchronous function call.
//!
//! # Two layers, and why the split is not cosmetic
//!
//! [`rules`] is ordinary Rust: it answers with [`WireError`], knows nothing
//! about JavaScript, and is tested on the host like anything else. The
//! `#[wasm_bindgen]` functions below are the shell — they translate, and that
//! is all they do.
//!
//! The split is load-bearing, and it cost a full test run to learn why:
//! `JsValue` is NOT inert off the wasm target. `JsValue::from_str` compiles
//! everywhere and then **aborts** when called on the host —
//! `STATUS_STACK_BUFFER_OVERRUN`, a non-unwinding panic that takes the whole
//! test binary with it, not a failure the harness can report. A host test that
//! exercised an error path therefore killed the run. With the rules answering
//! in plain Rust, they can be tested on the host, and the shell's translation
//! is exercised where it actually lives: by
//! `src/wasm/coreRules.parity.test.ts`, in wasm.
//!
//! # Where this crate ends up
//!
//! In the DESKTOP repository, once the UIs move out. It is the desktop's
//! binding to the core, and the desktop repo builds it as part of its own
//! build — a Tauri app is a Rust binary (`src-tauri`), so that repo carries a
//! Rust toolchain either way. The core repository stays what it is: domain
//! logic and the plugin system, shipping no artifacts of its own.
//!
//! That is why "pure marshalling" is a rule here and not a preference. When
//! this crate moves, **no rule may move with it**. Anything more than a
//! translation and a piece of the domain would leave the core, which is the
//! failure the whole split exists to prevent.
//!
//! `cal-ffi` is the contrast, and the reason to be strict: it is nominally the
//! mobile binding, but it holds the mobile `Host` — orchestration over
//! `host-core`, the registry, the cache and sync. That is logic, so `cal-ffi`
//! will have to be SPLIT rather than moved. This crate is designed so the same
//! question never arises for it.
//!
//! # Why this wraps so little
//!
//! It deliberately carries only rules with no policy attached — no i18n keys,
//! no glyphs. Both of those are open questions (does the core return keys or
//! prose? is `★` domain or typography?), and answering them by accident would
//! be deciding them.
//!
//! # The boundary is strings
//!
//! `TaskPriority` cannot carry `#[wasm_bindgen]`: it lives in `cal-core`, which
//! twelve adapter repositories compile against, and none of them should grow a
//! WebAssembly dependency. So the enum crosses as the same lowercase string
//! serde already writes on every wire in this app, and is parsed here. An
//! unknown value is an ERROR rather than a default: a silent fallback would
//! answer confidently for a value nobody wrote.

use wasm_bindgen::prelude::*;

pub mod rules;

use rules::WireError;

fn to_js(err: WireError) -> JsValue {
    JsValue::from_str(&err.to_string())
}

/// Does this task carry the TOP priority — the one level that survives in the
/// two-level system, where it is called "important" rather than "high"?
#[wasm_bindgen(js_name = isImportantPriority)]
pub fn is_important_priority(priority: &str) -> Result<bool, JsValue> {
    rules::is_important_priority(priority).map_err(to_js)
}

/// Sort rank for a priority: 0 sorts first.
///
/// The interesting one. Its callers are `Array.prototype.sort` comparators
/// (`src/components/BacklogRail.tsx`), which are synchronous by construction —
/// an async comparator returns promises, and every promise compares equal, so
/// the list would come out in arbitrary order. This is the call that decides
/// whether a rule can live in the core at all.
///
/// `scale` is the user's priority lens: `"three"` (low/medium/high) or `"two"`
/// (important / normal), from the synced `tasks.twoLevelPriority` setting.
#[wasm_bindgen(js_name = priorityRank)]
pub fn priority_rank(priority: &str, scale: &str) -> Result<u32, JsValue> {
    rules::priority_rank(priority, scale).map_err(to_js)
}

/// The priority a task gets when the user clears "important" in the two-level
/// system: whatever it already had, as long as that is not the top one.
///
/// Unchecking must not rewrite `low` into `medium`. Both read as "normal" and
/// look identical on every surface, so the write would change nothing the user
/// can see while changing what other clients — and the three-level system, if
/// they switch back — display. `undefined`/`null` means the task carried
/// nothing before; `medium` is the neutral answer.
#[wasm_bindgen(js_name = normalPriority)]
pub fn normal_priority(previous: Option<String>) -> Result<String, JsValue> {
    rules::normal_priority(previous.as_deref()).map_err(to_js)
}

/// Compare two NAMES — an account, a container, a contact, a day marker.
///
/// Case- and accent-insensitive: "Arbeit" and "arbeit" are one name to a
/// person, and a list mixing them must not split into two blocks. Replaces
/// `localeCompare(…, { sensitivity: 'base' })`.
///
/// `languageTag` is the language the USER chose in Aperio, not the one the
/// operating system reports. Every call this replaces passed `undefined` and
/// therefore followed the runtime — so someone reading Aperio in German on an
/// English system got English ordering.
#[wasm_bindgen(js_name = compareNames)]
pub fn compare_names(a: &str, b: &str, language_tag: &str) -> i32 {
    rules::names(a, b, language_tag)
}

/// Compare two TITLES — task titles, section names, anything the user typed.
///
/// Digit runs order by value ("Kapitel 2" before "Kapitel 10"), and case
/// separates, unlike [`compare_names`]. Replaces the bare `localeCompare` and
/// the `{ numeric: true }` one.
#[wasm_bindgen(js_name = compareTitles)]
pub fn compare_titles(a: &str, b: &str, language_tag: &str) -> i32 {
    rules::titles(a, b, language_tag)
}

/// Find the online meeting in an event, or answer `"null"`.
///
/// `sourcesJson` is `{providerField?, icalendarConference[], vendorProperties[],
/// location?, description?}` — the fields a detector looks at, in the order it
/// prefers them. The answer is a `ConferenceLink` as JSON, or the string
/// `"null"` when there is no meeting, which is what an ordinary appointment
/// gets and therefore the commonest answer of all.
///
/// The first thing to cross this boundary that is not a bare value. It still
/// crosses as a string: `serde_json` is already in this module's graph — the
/// core needs it for the extras codec — so JSON costs nothing here and gives
/// both doors one shape, which is what keeps them from drifting in what they
/// accept.
#[wasm_bindgen(js_name = detectConference)]
pub fn detect_conference(sources_json: &str) -> Result<String, JsValue> {
    rules::detect_conference(sources_json).map_err(to_js)
}

/// Copies worth offering among one day's rows.
///
/// `input_json` is `{events[], groups[], declines[]}`; the answer is a
/// `[{first, second}]` array of POSITIONS in `events`, because the caller is
/// already holding the rows and echoing them back would say nothing new.
#[wasm_bindgen(js_name = findGroupSuggestions)]
pub fn find_group_suggestions(input_json: &str) -> Result<String, JsValue> {
    rules::find_group_suggestions(input_json).map_err(to_js)
}

/// The row that most looks like a copy of an anchor.
///
/// `input_json` is `{anchor, candidates[]}`; the answer is the position in
/// `candidates`, or `"null"`.
#[wasm_bindgen(js_name = suggestGroupMate)]
pub fn suggest_group_mate(input_json: &str) -> Result<String, JsValue> {
    rules::suggest_group_mate(input_json).map_err(to_js)
}

/// Which rows of a window survive the meeting-duplicate filter.
///
/// The whole window crosses at once, and the answer is the POSITIONS that
/// stay. The rule this replaces reached the detection once per row — twice, in
/// fact — so a day view crossed this boundary forty times to answer one
/// question about forty rows.
#[wasm_bindgen(js_name = withoutDuplicateMeetings)]
pub fn without_duplicate_meetings(events_json: &str) -> Result<String, JsValue> {
    rules::without_duplicate_meetings(events_json).map_err(to_js)
}

/// The (meeting, appointment) pairs that should become groups.
///
/// `{events[], groups[], declines[]}` in, `[{meeting, event, joinUrl}]` out,
/// where `meeting` and `event` are POSITIONS in `events`. The join URL is
/// folded here, by a real URL parser — see `cal_core::normalize_join_url` for
/// what a hand-rolled fold would get wrong.
#[wasm_bindgen(js_name = findMeetingLinkPairs)]
pub fn find_meeting_link_pairs(input_json: &str) -> Result<String, JsValue> {
    rules::find_meeting_link_pairs(input_json).map_err(to_js)
}

/// Fold a join URL to what two spellings of the same link agree on.
///
/// Exported on its own because the pairing above is not the only thing that
/// may ever need the identity, and because a rule with no caller of its own is
/// hard to test through a door.
#[wasm_bindgen(js_name = normalizeJoinUrl)]
pub fn normalize_join_url(url: &str) -> String {
    rules::normalize_join_url(url)
}

/// Fold each group's members into a single row, keeping the input order.
///
/// `{events[], groups[]}` in, one row per surviving slot out — each naming the
/// POSITION of the event to draw, which is not always the slot's own.
#[wasm_bindgen(js_name = collapseEventGroups)]
pub fn collapse_event_groups(input_json: &str) -> Result<String, JsValue> {
    rules::collapse_event_groups(input_json).map_err(to_js)
}

/// What carrying an edit to a group's other copies would do.
#[wasm_bindgen(js_name = planCarry)]
pub fn plan_carry(input_json: &str) -> Result<String, JsValue> {
    rules::plan_carry(input_json).map_err(to_js)
}

/// The fields of the standalone row a carried OCCURRENCE edit creates, or
/// `"null"` when the instant cannot be read.
#[wasm_bindgen(js_name = occurrenceCarryFields)]
pub fn occurrence_carry_fields(input_json: &str) -> Result<String, JsValue> {
    rules::occurrence_carry_fields(input_json).map_err(to_js)
}

/// The fields of the row a carried "this and all following" edit creates, or
/// `"null"` when the cut point cannot be read.
#[wasm_bindgen(js_name = futureCarryFields)]
pub fn future_carry_fields(input_json: &str) -> Result<String, JsValue> {
    rules::future_carry_fields(input_json).map_err(to_js)
}

/// The carried fields laid over a member's own current values.
#[wasm_bindgen(js_name = carryOntoFields)]
pub fn carry_onto(input_json: &str) -> Result<String, JsValue> {
    rules::carry_onto(input_json).map_err(to_js)
}

/// The task view's rows: which group each task lands in, in which order,
/// under which header, at what depth. Asked inside `useMemo`, so synchronous.
#[wasm_bindgen(js_name = groupTasks)]
pub fn group_tasks(input_json: &str) -> Result<String, JsValue> {
    rules::group_tasks(input_json).map_err(to_js)
}

/// The tasks a calendar day shows, with what each chip carries. Asked inside `useMemo`, so synchronous.
#[wasm_bindgen(js_name = tasksOnDays)]
pub fn tasks_on_days(input_json: &str) -> Result<String, JsValue> {
    rules::tasks_on_days(input_json).map_err(to_js)
}
/// The two calendar weeks the backlog rail splits its deadlines into.
#[wasm_bindgen(js_name = backlogWeeks)]
pub fn backlog_weeks(input_json: &str) -> Result<String, JsValue> {
    rules::backlog_weeks(input_json).map_err(to_js)
}
/// Deadline-carrying items by week, as positions.
#[wasm_bindgen(js_name = splitDeadlinesByWeek)]
pub fn split_deadlines_by_week(input_json: &str) -> Result<String, JsValue> {
    rules::split_deadlines_by_week(input_json).map_err(to_js)
}

/// Every i18n key of the task vocabulary, as one table: read once at
/// install, never asked per chip.
#[wasm_bindgen(js_name = taskI18nKeys)]
pub fn task_i18n_keys() -> String {
    rules::task_i18n_keys()
}

/// How far every parent's subtasks are, for a whole task list at once.
#[wasm_bindgen(js_name = subtaskProgress)]
pub fn subtask_progress(input_json: &str) -> Result<String, JsValue> {
    rules::subtask_progress(input_json).map_err(to_js)
}
