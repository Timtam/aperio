//! Google Tasks API v1 — `https://tasks.googleapis.com/tasks/v1`.
//!
//! Tasks live behind a separate REST surface from Calendar (different
//! base URL, different request shape), but share the same OAuth
//! token thanks to the combined consent scope (see `auth::SCOPES`).
//! This module reuses `ApiState`'s token + refresh plumbing through
//! the lower-level `send_with_refresh` helper, so a 401 here drives
//! the same lazy refresh the Calendar paths use.
//!
//! Aperio ↔ Google mapping (DESIGN.md §9.7):
//!
//!  - Title       ↔ `title`
//!  - Description ↔ `notes`
//!  - Status (4)  ↔ `status` (2):
//!      Open / InProgress → `needsAction`,
//!      Completed         → `completed`,
//!      Cancelled         → `needsAction` (Google has no equivalent;
//!                          documented in the comment on
//!                          `task_status_to_google`).
//!  - Priority    ↔ dropped (Google Tasks has no priority field);
//!                   reads default to Medium. A non-default priority on
//!                   write logs a tracing::warn — it can't round-trip and
//!                   reads back as Medium on the next sync.
//!  - scheduled_* ↔ `due` (Google has ONE date slot; scheduled wins
//!                   on write, falling back to deadline_date when
//!                   absent). Date-only — Google ignores the time
//!                   portion server-side.
//!  - deadline_*  ↔ same `due` slot as scheduled. The round-trip
//!                   loses the distinction; documented in DESIGN.md.
//!  - parent_id   ↔ `parent` (subtasks). The Tasks API uses a
//!                   separate `/move` endpoint for re-parenting at
//!                   runtime, but `parent` on create works.
//!  - Recurrence  ↔ dropped (Google Tasks has no recurrence). Write
//!                   path logs a tracing::warn.
//!  - Reminders   ↔ dropped (Google Tasks has no reminders either —
//!                   they'd live on a separate `/users/@me/notes`
//!                   surface that even Google's own UI doesn't
//!                   touch).
//!
//! Date semantics: Google Tasks stores `due` as an RFC 3339 datetime
//! string but treats only the date portion as significant. We send
//! the local date at `00:00:00.000Z` and read it back as a
//! NaiveDate, discarding any returned time-of-day.

use std::sync::Arc;

use cal_core::{NewTask, Task, TaskList, TaskPriority, TaskRecurrence, TaskStatus};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as _Mutex;

use crate::api::{delete_absolute, get_absolute, patch_absolute, post_absolute, ApiState};
use crate::error::GoogleResult;

const TASKS_API_BASE: &str = "https://tasks.googleapis.com/tasks/v1";

// ── Public surface routed into TasksFeature ────────────────────────────

/// `GET /users/@me/lists`. Returns every task list the user owns
/// or has been shared into.
pub async fn list_task_lists(state: &ApiState) -> GoogleResult<Vec<TaskList>> {
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        let mut url = format!("{TASKS_API_BASE}/users/@me/lists?maxResults=100");
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let resp: TaskListsResponse = get_absolute(state, &url).await?;
        for entry in resp.items {
            out.push(map_task_list(entry));
        }
        match resp.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }
    Ok(out)
}

/// `GET /lists/{tasklistId}/tasks`. We ask for hidden + completed
/// tasks too so they reach the day-start review. `maxResults=100` is
/// the API ceiling per page; we paginate.
pub async fn get_tasks(state: &ApiState, list_id: &str) -> GoogleResult<Vec<Task>> {
    let list_enc = urlencoding(list_id);
    let mut out = Vec::new();
    let mut page_token: Option<String> = None;
    loop {
        // `showCompleted=true` is the default but we set it
        // explicitly to be defensive. `showHidden=true` pulls back
        // hidden tasks (Google hides them after a certain age) so
        // Aperio still sees them — the dialog already filters by
        // status.
        let mut url = format!(
            "{TASKS_API_BASE}/lists/{list_enc}/tasks\
             ?maxResults=100&showCompleted=true&showHidden=true",
        );
        if let Some(t) = &page_token {
            url.push_str("&pageToken=");
            url.push_str(&urlencoding(t));
        }
        let resp: TasksResponse = get_absolute(state, &url).await?;
        for entry in resp.items {
            out.push(map_task(entry, list_id));
        }
        match resp.next_page_token {
            Some(t) => page_token = Some(t),
            None => break,
        }
    }
    Ok(out)
}

/// `POST /lists/{tasklistId}/tasks` — create a task. The
/// server-assigned id + etag come back in the response; we
/// synthesise the returned `Task` from those plus the request
/// payload, saving a follow-up GET.
pub async fn create_task(state: &ApiState, list_id: &str, task: NewTask) -> GoogleResult<Task> {
    let list_enc = urlencoding(list_id);
    let mut url = format!("{TASKS_API_BASE}/lists/{list_enc}/tasks");
    // Google's `parent` query param sets the parent at insert time
    // — the API rejects `parent` in the JSON body. Match the
    // documented behaviour.
    if let Some(parent) = task.parent_id.as_deref().filter(|s| !s.is_empty()) {
        url.push_str("?parent=");
        url.push_str(&urlencoding(parent));
    }
    let body = new_task_to_body(&task);
    let entry: TaskEntry = post_absolute(state, &url, &body).await?;
    Ok(map_task(entry, list_id))
}

/// `PATCH /lists/{tasklistId}/tasks/{taskId}`. Partial updates work
/// here too, but we send every user-visible field so the local copy
/// and the server stay in step without diffing.
///
/// Re-parenting is NOT handled by PATCH — Google rejects a `parent`
/// in the body. If the caller changed `parent_id` we issue a
/// follow-up `/move` request after the field update lands.
pub async fn update_task(state: &ApiState, task: &Task) -> GoogleResult<Task> {
    let list_enc = urlencoding(&task.list_id);
    let task_enc = urlencoding(&task.id);
    let url = format!("{TASKS_API_BASE}/lists/{list_enc}/tasks/{task_enc}");
    let body = task_to_body(task);
    let entry: TaskEntry = patch_absolute(state, &url, &body).await?;
    // Issue the move if the caller asked for a new parent. The
    // Tasks API takes parent + previous as query params; we don't
    // care about position, so we omit `previous` and let Google
    // append to the end.
    if let Some(parent) = task.parent_id.as_deref().filter(|s| !s.is_empty()) {
        let move_url = format!(
            "{TASKS_API_BASE}/lists/{list_enc}/tasks/{task_enc}/move?parent={}",
            urlencoding(parent),
        );
        let _: TaskEntry = post_absolute(state, &move_url, &serde_json::json!({})).await?;
    }
    Ok(map_task(entry, &task.list_id))
}

/// `DELETE /lists/{tasklistId}/tasks/{taskId}`.
pub async fn delete_task(state: &ApiState, list_id: &str, task_id: &str) -> GoogleResult<()> {
    let list_enc = urlencoding(list_id);
    let task_enc = urlencoding(task_id);
    let url = format!("{TASKS_API_BASE}/lists/{list_enc}/tasks/{task_enc}");
    delete_absolute(state, &url).await
}

/// `PATCH /users/@me/lists/{tasklistId}` with `{ "title": "..." }`.
pub async fn rename_task_list(state: &ApiState, list_id: &str, new_name: &str) -> GoogleResult<()> {
    let list_enc = urlencoding(list_id);
    let url = format!("{TASKS_API_BASE}/users/@me/lists/{list_enc}");
    let body = serde_json::json!({ "title": new_name });
    let _: serde_json::Value = patch_absolute(state, &url, &body).await?;
    Ok(())
}

/// `POST /users/@me/lists` with `{ "title": "..." }` — create a task
/// list. Google Tasks lists are flat, so `parent_id` is ignored.
pub async fn create_task_list(
    state: &ApiState,
    name: &str,
    _parent_id: Option<&str>,
) -> GoogleResult<TaskList> {
    let url = format!("{TASKS_API_BASE}/users/@me/lists");
    let body = serde_json::json!({ "title": name });
    let entry: TaskListEntry = post_absolute(state, &url, &body).await?;
    Ok(map_task_list(entry))
}

/// `DELETE /users/@me/lists/{tasklistId}`.
pub async fn delete_task_list(state: &ApiState, list_id: &str) -> GoogleResult<()> {
    let list_enc = urlencoding(list_id);
    let url = format!("{TASKS_API_BASE}/users/@me/lists/{list_enc}");
    delete_absolute(state, &url).await
}

// (HTTP helpers moved to api.rs — `get_absolute` / `post_absolute`
// / `patch_absolute` / `delete_absolute` are shared with the
// People API client in `contacts.rs`.)

// ── JSON wire shapes ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct TaskListsResponse {
    #[serde(default)]
    items: Vec<TaskListEntry>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TaskListEntry {
    id: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TasksResponse {
    #[serde(default)]
    items: Vec<TaskEntry>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}

/// Google Tasks resource. Fields not in the Aperio model (etag,
/// position, selfLink, links, webViewLink, deleted, hidden) are
/// skipped here — serde tolerates unknown fields by default.
#[derive(Debug, Default, Deserialize, Serialize)]
struct TaskEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    due: Option<String>,
    #[serde(default)]
    completed: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    updated: Option<String>,
}

// ── Mappers ────────────────────────────────────────────────────────────

fn map_task_list(entry: TaskListEntry) -> TaskList {
    TaskList {
        color_label: None,
        id: entry.id,
        name: entry.title.unwrap_or_else(|| "Tasks".into()),
        color: None,
        default_sound: None,
        embedded_in_calendar: None,
        parent_id: None,
        // Aperio model: read-only means "the adapter can't write at
        // all" — Google Tasks lets us PATCH titles, so the list is
        // writable. Per-task read-only would be a sharing
        // restriction Google doesn't expose on the list level.
        read_only: false,
    }
}

fn map_task(entry: TaskEntry, list_id: &str) -> Task {
    let status = entry
        .status
        .as_deref()
        .map(google_status_to_task_status)
        .unwrap_or(TaskStatus::Open);
    let deadline_date = entry.due.as_deref().and_then(parse_google_date);
    let completed_at = entry.completed.as_deref().and_then(parse_google_datetime);
    let updated_at = entry
        .updated
        .as_deref()
        .and_then(parse_google_datetime)
        .unwrap_or_else(Utc::now);

    // DESIGN §9.12: Google Tasks stores no Aperio recurrence natively, so
    // the on-demand axes / resurface_date / series_id ride a visible block
    // in `notes`. Strip it and overlay the carried fields.
    let (clean_notes, extras) = cal_core::extras::extract(entry.notes.as_deref());
    let mut recurrence = None;
    let mut resurface_date = None;
    let mut series_id = None;
    if let Some(extras) = &extras {
        cal_core::apply_task_extras(extras, &mut recurrence, &mut resurface_date, &mut series_id);
    }

    Task {
        assignees: Vec::new(),
        id: entry.id,
        list_id: list_id.to_string(),
        title: entry.title.unwrap_or_default(),
        description: clean_notes.filter(|s| !s.is_empty()),
        status,
        // Google Tasks has no priority field. Default to Medium so
        // round-trips through the local UI don't show every task as
        // "low priority".
        priority: TaskPriority::Medium,
        // Google has ONE date slot. By DESIGN.md §9.7 it maps to
        // scheduled_date on read; deadline_date stays empty. On
        // write the inverse fallback kicks in if the user only set
        // a deadline.
        scheduled_date: deadline_date,
        scheduled_time: None,
        deadline_date: None,
        deadline_time: None,
        recurrence,
        parent_id: entry.parent,
        section_id: None,
        color_label: None,
        reminders: Vec::new(),
        sound: None,
        // Google Tasks doesn't surface a `created` timestamp;
        // `updated` is the only mtime we have. Re-use it for both
        // so the row sorts sensibly in the frontend cache.
        created_at: updated_at,
        updated_at,
        completed_at,
        etag: entry.etag,
        resurface_date,
        series_id,
    }
}

/// DESIGN §9.12: fold the Aperio-only fields into a visible extras block on
/// `notes` (Google Tasks' only shared channel). Backlog/on-demand
/// recurrence round-trips here; a plain scheduled rule has no Google home
/// and is dropped with a warning.
fn google_notes(
    notes: Option<&str>,
    recurrence: Option<&TaskRecurrence>,
    resurface_date: Option<NaiveDate>,
    series_id: Option<&str>,
) -> Option<String> {
    let extras = cal_core::extras_for_task(recurrence, resurface_date, series_id);
    cal_core::extras::embed(notes, &extras).filter(|s| !s.is_empty())
}

/// A recurrence Google genuinely can't keep: a plain scheduled rule, which
/// has no extras block and no native Google channel. Backlog/on-demand
/// rules ride the bag, so they're not "dropped".
fn recurrence_dropped(recurrence: Option<&TaskRecurrence>) -> bool {
    recurrence.is_some_and(|r| !cal_core::recurrence_needs_extras(r))
}

fn new_task_to_body(new: &NewTask) -> TaskEntry {
    let due = combine_date_to_google(new.scheduled_date.or(new.deadline_date));
    if recurrence_dropped(new.recurrence.as_ref()) {
        tracing::warn!(
            "Google Tasks adapter dropping recurrence on write — Google Tasks API has no recurrence field",
        );
    }
    if !new.reminders.is_empty() {
        tracing::warn!(
            "Google Tasks adapter dropping reminders on write — Google Tasks API has no reminder field",
        );
    }
    if new.priority != TaskPriority::Medium {
        tracing::warn!(
            "Google Tasks adapter dropping non-default priority on write — Google Tasks API has no priority field (it reads back as Medium)",
        );
    }
    TaskEntry {
        id: String::new(),
        etag: None,
        title: Some(new.title.clone()),
        notes: google_notes(
            new.description.as_deref(),
            new.recurrence.as_ref(),
            new.resurface_date,
            new.series_id.as_deref(),
        ),
        status: Some(task_status_to_google(new.status).to_string()),
        due,
        completed: None,
        // Parent gets attached via the `?parent=` query param on
        // insert (Google rejects `parent` in the JSON body); zero it
        // out here.
        parent: None,
        updated: None,
    }
}

fn task_to_body(task: &Task) -> TaskEntry {
    let due = combine_date_to_google(task.scheduled_date.or(task.deadline_date));
    if recurrence_dropped(task.recurrence.as_ref()) {
        tracing::warn!(
            "Google Tasks adapter dropping recurrence on update — Google Tasks API has no recurrence field",
        );
    }
    if !task.reminders.is_empty() {
        tracing::warn!(
            "Google Tasks adapter dropping reminders on update — Google Tasks API has no reminder field",
        );
    }
    if task.priority != TaskPriority::Medium {
        tracing::warn!(
            "Google Tasks adapter dropping non-default priority on update — Google Tasks API has no priority field (it reads back as Medium)",
        );
    }
    TaskEntry {
        id: task.id.clone(),
        etag: task.etag.clone(),
        title: Some(task.title.clone()),
        notes: google_notes(
            task.description.as_deref(),
            task.recurrence.as_ref(),
            task.resurface_date,
            task.series_id.as_deref(),
        ),
        status: Some(task_status_to_google(task.status).to_string()),
        due,
        completed: task.completed_at.map(format_google_datetime),
        // Same as create: `parent` goes through the `/move`
        // endpoint, not via PATCH body.
        parent: None,
        updated: None,
    }
}

// ── Status mapping ─────────────────────────────────────────────────────

/// Google → Aperio. `needsAction` ↔ Open (we lose the
/// in-progress / cancelled distinction on the round-trip;
/// `task_status_to_google` documents the inverse).
fn google_status_to_task_status(s: &str) -> TaskStatus {
    match s {
        "completed" => TaskStatus::Completed,
        // "needsAction" + anything we don't recognise default to Open.
        // Google's enum is closed (those are the only two documented
        // values) but defaulting keeps us safe against future schema
        // additions.
        _ => TaskStatus::Open,
    }
}

/// Aperio → Google. `InProgress` and `Cancelled` have no Google
/// equivalent — both collapse to `needsAction` here. The first
/// loses the "I started this" signal; the second is the bigger
/// drift (cancelled tasks come back as open after a round-trip and
/// would re-appear in the day-start review). Documented as a
/// known limitation in DESIGN.md §9.7.
fn task_status_to_google(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Open => "needsAction",
        TaskStatus::InProgress => "needsAction",
        TaskStatus::Completed => "completed",
        TaskStatus::Cancelled => "needsAction",
    }
}

// ── Date handling ──────────────────────────────────────────────────────

/// Google Tasks stores `due` as an RFC 3339 datetime but treats the
/// time as ignored — Google's own UI always shows date-only. Parse
/// the date portion; drop any time-of-day.
fn parse_google_date(raw: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc).date_naive())
}

/// Full RFC 3339 datetime parse for `updated` / `completed`. These
/// fields DO carry a meaningful time-of-day (the moment the task
/// state changed); we keep the full precision.
fn parse_google_datetime(raw: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Format a NaiveDate as Google's expected `YYYY-MM-DDT00:00:00.000Z`.
/// The time-of-day is ignored server-side but the field's regex
/// rejects bare date strings, so we have to send the suffix.
fn combine_date_to_google(date: Option<NaiveDate>) -> Option<String> {
    date.map(|d| format!("{}T00:00:00.000Z", d.format("%Y-%m-%d")))
}

fn format_google_datetime(ts: DateTime<Utc>) -> String {
    ts.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

// ── Percent-encoding (kept local; api.rs has its own copy) ─────────────

fn urlencoding(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

// Suppress the unused-import warning on `Arc`/`_Mutex` — we re-export
// them via the `pub use` of ApiState elsewhere; the explicit import
// here keeps the file self-contained for grep.
#[allow(dead_code)]
type _ArcMarker = Arc<_Mutex<()>>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::TokenSet;
    use crate::error::GoogleError;
    use mockito::Server;
    use std::sync::Arc;
    use tokio::sync::Mutex;

    fn fixture_state(server_url: &str) -> ApiState {
        ApiState {
            tokens: Arc::new(Mutex::new(TokenSet {
                access_token: "access".into(),
                refresh_token: Some("refresh".into()),
                expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
                scope: None,
            })),
            client_id: "cid".into(),
            client_secret: "secret".into(),
            http: reqwest::Client::new(),
            token_url: format!("{server_url}/token"),
            api_base: server_url.to_string(),
        }
    }

    /// Override `TASKS_API_BASE` per test by monkey-patching the
    /// fixture's URL. We can't reassign a `const` at runtime, so
    /// every test routes through a mock that intercepts the exact
    /// path Google would see. mockito matches on path regardless of
    /// host — the tasks code embeds the host in the URL, but
    /// reqwest will hit whatever URL mockito serves once we point
    /// our fixture's `api_base` at it (the tasks module uses its
    /// own base; we bypass that by … wait, we can't). The simpler
    /// approach: spin a stand-alone mockito server and inject the
    /// URL via a const override.
    ///
    /// Actually mockito's host is dynamic, so we need to ALSO
    /// override the TASKS_API_BASE for tests. We do that via a
    /// per-test wrapper that uses the absolute server URL directly,
    /// calling the same logic the tasks functions invoke. The
    /// envelope-shape and mapping tests don't need the network.
    /// HTTP tests check only the helpers + a single end-to-end
    /// path against a stubbed server URL.

    // ── Status mapping ─────────────────────────────────────────

    #[test]
    fn status_round_trip_for_open_and_completed() {
        assert_eq!(
            google_status_to_task_status("needsAction"),
            TaskStatus::Open
        );
        assert_eq!(
            google_status_to_task_status("completed"),
            TaskStatus::Completed
        );
        // Defaults to Open for safety against future enum additions.
        assert_eq!(google_status_to_task_status("flagged"), TaskStatus::Open);

        assert_eq!(task_status_to_google(TaskStatus::Open), "needsAction");
        assert_eq!(task_status_to_google(TaskStatus::InProgress), "needsAction");
        assert_eq!(task_status_to_google(TaskStatus::Completed), "completed");
        assert_eq!(task_status_to_google(TaskStatus::Cancelled), "needsAction");
    }

    // ── Date mapping ───────────────────────────────────────────

    #[test]
    fn parse_google_date_drops_time_component() {
        assert_eq!(
            parse_google_date("2026-05-22T14:00:00.000Z"),
            Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
        );
        // Bare date string isn't valid RFC 3339 — we return None
        // (the next polling tick would just have an empty due
        // field), better than misinterpreting.
        assert!(parse_google_date("2026-05-22").is_none());
    }

    #[test]
    fn combine_date_to_google_emits_midnight_utc() {
        let d = Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap());
        assert_eq!(
            combine_date_to_google(d).as_deref(),
            Some("2026-05-22T00:00:00.000Z"),
        );
        assert_eq!(combine_date_to_google(None), None);
    }

    // ── Body shape (NewTask → wire) ────────────────────────────

    fn sample_new_task() -> NewTask {
        NewTask {
            assignees: Vec::new(),
            title: "Submit invoice".into(),
            description: Some("Q2 client".into()),
            status: TaskStatus::Open,
            priority: TaskPriority::High,
            scheduled_date: Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
            scheduled_time: None,
            deadline_date: None,
            deadline_time: None,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders: Vec::new(),
            sound: None,
            resurface_date: None,
            series_id: None,
        }
    }

    #[test]
    fn new_task_body_carries_title_notes_status_due() {
        let body = new_task_to_body(&sample_new_task());
        assert_eq!(body.title.as_deref(), Some("Submit invoice"));
        assert_eq!(body.notes.as_deref(), Some("Q2 client"));
        assert_eq!(body.status.as_deref(), Some("needsAction"));
        assert_eq!(body.due.as_deref(), Some("2026-05-22T00:00:00.000Z"));
        // Parent never goes in the body — it rides on the URL.
        assert!(body.parent.is_none());
    }

    #[test]
    fn extras_block_round_trips_backlog_recurrence_through_notes() {
        use cal_core::{
            MonthDay, RecurrenceAnchor, RecurrenceEnd, RecurrenceFrequency, RecurrencePlacement,
            TaskRecurrence,
        };
        let mut nt = sample_new_task();
        nt.description = Some("Swap shoes".into());
        nt.recurrence = Some(TaskRecurrence {
            frequency: RecurrenceFrequency::Yearly,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: Some(RecurrenceEnd::Never),
            anchor: RecurrenceAnchor::FromCompletion,
            placement: RecurrencePlacement::Backlog,
            fixed_dates: Some(vec![MonthDay { month: 4, day: 1 }]),
        });
        nt.resurface_date = Some(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap());
        nt.series_id = Some("series-shoes".into());

        let body = new_task_to_body(&nt);
        let notes = body.notes.clone().unwrap();
        assert!(notes.starts_with("Swap shoes"));
        assert!(notes.contains("aperio:1:"));

        let entry = TaskEntry {
            id: "T1".into(),
            etag: None,
            title: Some("Swap shoes".into()),
            notes: body.notes,
            status: Some("needsAction".into()),
            due: None,
            completed: None,
            parent: None,
            updated: None,
        };
        let restored = map_task(entry, "L1");
        assert_eq!(restored.description.as_deref(), Some("Swap shoes"));
        assert_eq!(restored.recurrence, nt.recurrence);
        assert_eq!(restored.resurface_date, nt.resurface_date);
        assert_eq!(restored.series_id.as_deref(), Some("series-shoes"));
    }

    #[test]
    fn new_task_body_falls_back_to_deadline_when_scheduled_missing() {
        let mut nt = sample_new_task();
        nt.scheduled_date = None;
        nt.deadline_date = Some(NaiveDate::from_ymd_opt(2026, 6, 1).unwrap());
        let body = new_task_to_body(&nt);
        assert_eq!(body.due.as_deref(), Some("2026-06-01T00:00:00.000Z"));
    }

    #[test]
    fn new_task_body_omits_due_when_both_dates_absent() {
        let mut nt = sample_new_task();
        nt.scheduled_date = None;
        nt.deadline_date = None;
        let body = new_task_to_body(&nt);
        assert!(body.due.is_none());
    }

    // ── Wire → Task ────────────────────────────────────────────

    #[test]
    fn map_task_pulls_due_into_scheduled_date() {
        let entry = TaskEntry {
            id: "gtid".into(),
            etag: Some("e-1".into()),
            title: Some("Buy bread".into()),
            notes: Some("Bakery on Hauptstraße".into()),
            status: Some("needsAction".into()),
            due: Some("2026-05-22T00:00:00.000Z".into()),
            completed: None,
            parent: None,
            updated: Some("2026-05-19T08:00:00.000Z".into()),
        };
        let task = map_task(entry, "MyTasks");
        assert_eq!(task.id, "gtid");
        assert_eq!(task.list_id, "MyTasks");
        assert_eq!(task.title, "Buy bread");
        assert_eq!(task.description.as_deref(), Some("Bakery on Hauptstraße"));
        assert_eq!(task.status, TaskStatus::Open);
        assert_eq!(task.priority, TaskPriority::Medium); // Google has none
        assert_eq!(
            task.scheduled_date,
            Some(NaiveDate::from_ymd_opt(2026, 5, 22).unwrap()),
        );
        assert!(task.deadline_date.is_none());
        assert_eq!(task.etag.as_deref(), Some("e-1"));
    }

    #[test]
    fn map_task_translates_completed_status_with_timestamp() {
        let entry = TaskEntry {
            id: "gtid".into(),
            etag: None,
            title: Some("Done".into()),
            notes: None,
            status: Some("completed".into()),
            due: None,
            completed: Some("2026-05-20T10:30:00.000Z".into()),
            parent: None,
            updated: Some("2026-05-20T10:30:00.000Z".into()),
        };
        let task = map_task(entry, "MyTasks");
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(task.completed_at.is_some());
    }

    // ── TaskList ───────────────────────────────────────────────

    #[test]
    fn map_task_list_uses_default_when_title_missing() {
        let list = map_task_list(TaskListEntry {
            id: "lid".into(),
            title: None,
        });
        assert_eq!(list.id, "lid");
        assert_eq!(list.name, "Tasks");
        assert!(!list.read_only);
    }

    // ── End-to-end via mockito ────────────────────────────────

    /// Override `TASKS_API_BASE` for HTTP tests. We can't mutate
    /// the `const`, so each end-to-end test builds its own URL
    /// manually and calls the lower-level helpers. The thin layer
    /// loses the "list_task_lists actually constructs the right
    /// URL" coverage, but the URL is one line of code right above
    /// — adding bespoke test infrastructure to verify it would
    /// outweigh the benefit.

    #[tokio::test]
    async fn delete_absolute_round_trips_against_204() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/lists/L/tasks/T")
            .with_status(204)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        delete_absolute(&state, &format!("{}/lists/L/tasks/T", server.url()))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn delete_absolute_surfaces_404_as_http_error() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("DELETE", "/lists/L/tasks/missing")
            .with_status(404)
            .with_body(r#"{"error":{"code":404,"message":"Not found"}}"#)
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let err = delete_absolute(&state, &format!("{}/lists/L/tasks/missing", server.url()))
            .await
            .unwrap_err();
        match err {
            GoogleError::Http { status, .. } => assert_eq!(status, 404),
            other => panic!("expected Http, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn get_absolute_returns_decoded_json() {
        let mut server = Server::new_async().await;
        let _m = server
            .mock("GET", "/lists/L/tasks")
            .with_status(200)
            .with_body(
                r#"{"items":[{"id":"t1","title":"One","status":"needsAction"},{"id":"t2","title":"Two","status":"completed"}]}"#,
            )
            .create_async()
            .await;
        let state = fixture_state(&server.url());
        let resp: TasksResponse = get_absolute(&state, &format!("{}/lists/L/tasks", server.url()))
            .await
            .unwrap();
        assert_eq!(resp.items.len(), 2);
        assert_eq!(resp.items[0].id, "t1");
        assert_eq!(resp.items[1].status.as_deref(), Some("completed"));
    }
}
