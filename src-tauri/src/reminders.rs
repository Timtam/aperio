//! Local reminder scheduler (DESIGN.md section 14).
//!
//! Single background tokio task that
//!   1. enumerates the upcoming reminder triggers across every event
//!      and task in the local database,
//!   2. sleeps until the next one fires (or until something signals
//!      that the schedule has changed),
//!   3. dispatches an OS notification via `tauri-plugin-notification`
//!      and records the fire so a restart within the same minute does
//!      not double-notify.
//!
//! Re-computation is signalled through a [`tokio::sync::Notify`]. The
//! CRUD command layer calls [`ReminderScheduler::invalidate`] after
//! any event or task mutation; the worker wakes up, throws away its
//! pending wait, and re-scans the DB.
//!
//! Storage of "already fired" reminders is in-memory only. A crash
//! between a fire and the next scan can re-deliver a reminder; the
//! sync wave (Phase 7) will persist it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cal_adapter_local::SharedConn;
use cal_core::{Reminder, ReminderKind};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use rusqlite::params;
use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Notify;
use tracing::{info, warn};

/// Identifier for a single fired reminder. Two reminders fire at the
/// "same" trigger only if they share item id, kind, AND timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FiredKey {
    item_id: String,
    trigger_iso: String,
}

/// One concrete reminder occurrence, after expanding recurrence and
/// resolving relative offsets to absolute UTC times.
#[derive(Debug, Clone)]
struct Trigger {
    item_id: String,
    item_kind: ItemKind,
    title: String,
    body: String,
    trigger_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Event,
    Task,
}

/// Public DTO for the reminders overview dialog (Ctrl+Shift+R).
#[derive(Debug, Clone, Serialize)]
pub struct UpcomingReminder {
    pub item_id: String,
    pub item_kind: ItemKind,
    pub title: String,
    /// Trigger timestamp in RFC 3339 / ISO 8601 UTC.
    pub trigger_at: String,
}

pub struct ReminderScheduler {
    db: SharedConn,
    invalidate: Arc<Notify>,
    fired: Arc<Mutex<HashSet<FiredKey>>>,
}

impl ReminderScheduler {
    /// Start the worker loop on Tauri's async runtime.
    ///
    /// We can't use `tokio::spawn` here because `tauri::Builder::setup`
    /// fires on the main thread without an active tokio runtime
    /// context. `tauri::async_runtime::spawn` resolves against the
    /// runtime Tauri installs itself (tokio by default), which is the
    /// same runtime that powers `#[tauri::command]` async handlers —
    /// so the `tokio::select!` and `tokio::time::sleep` calls inside
    /// the worker still work as expected.
    pub fn spawn<R: Runtime>(db: SharedConn, app: AppHandle<R>) -> Arc<Self> {
        let scheduler = Arc::new(Self {
            db,
            invalidate: Arc::new(Notify::new()),
            fired: Arc::new(Mutex::new(HashSet::new())),
        });
        let worker = scheduler.clone();
        tauri::async_runtime::spawn(async move {
            // First sweep covers the "fired while we were offline"
            // case for app_start reminders.
            worker.fire_app_start_reminders(&app);
            worker.worker_loop(app).await;
        });
        scheduler
    }

    /// Wake the worker so it re-scans the DB. Safe to call from any
    /// thread; multiple back-to-back calls are coalesced by `Notify`.
    pub fn invalidate(&self) {
        self.invalidate.notify_one();
    }

    /// Snapshot reminder triggers for the Ctrl+Shift+R overview
    /// dialog. Includes both already-passed and upcoming triggers
    /// within a generous window so the user can review what fired
    /// recently and what is still pending. Sorted ascending by
    /// trigger time and capped at `limit` to keep the dialog snappy.
    pub fn upcoming(&self, limit: usize) -> Vec<UpcomingReminder> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::days(OVERVIEW_PAST_DAYS);
        let latest = now + ChronoDuration::days(OVERVIEW_FUTURE_DAYS);
        let mut triggers = self.collect_triggers_in_window(earliest, latest);
        triggers.sort_by_key(|t| t.trigger_at);
        triggers
            .into_iter()
            .take(limit)
            .map(|t| UpcomingReminder {
                item_id: t.item_id,
                item_kind: t.item_kind,
                title: t.title,
                trigger_at: t.trigger_at.to_rfc3339(),
            })
            .collect()
    }

    async fn worker_loop<R: Runtime>(self: Arc<Self>, app: AppHandle<R>) {
        loop {
            let triggers = self.collect_pending_triggers();
            let next = triggers.into_iter().min_by_key(|t| t.trigger_at);

            match next {
                Some(t) => {
                    let now = Utc::now();
                    let delta = t.trigger_at - now;
                    let dur = if delta <= ChronoDuration::zero() {
                        Duration::ZERO
                    } else {
                        delta.to_std().unwrap_or(Duration::from_secs(60))
                    };
                    tokio::select! {
                        _ = tokio::time::sleep(dur) => {
                            self.fire(&app, &t);
                        }
                        _ = self.invalidate.notified() => {
                            // schedule changed, recompute
                            continue;
                        }
                    }
                }
                None => {
                    // Nothing scheduled — block until the next mutation.
                    self.invalidate.notified().await;
                }
            }
        }
    }

    /// Walk events + tasks once and collect every reminder trigger
    /// whose absolute trigger time falls inside `[earliest, latest]`.
    /// No filtering for "already fired" — callers layer that on top.
    fn collect_triggers_in_window(
        &self,
        earliest: DateTime<Utc>,
        latest: DateTime<Utc>,
    ) -> Vec<Trigger> {
        let conn = match self.db.lock() {
            Ok(c) => c,
            Err(err) => {
                warn!(?err, "reminder DB mutex poisoned");
                return Vec::new();
            }
        };
        let mut acc: Vec<Trigger> = Vec::new();

        // Events
        if let Ok(mut stmt) = conn.prepare(EVENT_QUERY) {
            if let Ok(mut rows) = stmt.query(params![]) {
                while let Some(row) = rows.next().unwrap_or(None) {
                    let id: String = row.get(0).unwrap_or_default();
                    let title: String = row.get(1).unwrap_or_default();
                    let start_str: String = row.get(2).unwrap_or_default();
                    let reminders_json: Option<String> = row.get(3).unwrap_or(None);
                    let Some(reminders) = parse_reminders(reminders_json.as_deref())
                    else {
                        continue;
                    };
                    let Ok(start) = start_str.parse::<DateTime<Utc>>() else {
                        continue;
                    };
                    for r in &reminders {
                        if let Some(at) = trigger_time_for(&r.kind, start) {
                            if at >= earliest && at <= latest {
                                acc.push(Trigger {
                                    item_id: id.clone(),
                                    item_kind: ItemKind::Event,
                                    title: title.clone(),
                                    body: format_event_body(&start),
                                    trigger_at: at,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Tasks
        if let Ok(mut stmt) = conn.prepare(TASK_QUERY) {
            if let Ok(mut rows) = stmt.query(params![]) {
                while let Some(row) = rows.next().unwrap_or(None) {
                    let id: String = row.get(0).unwrap_or_default();
                    let title: String = row.get(1).unwrap_or_default();
                    let scheduled_date: Option<String> = row.get(2).unwrap_or(None);
                    let deadline_date: Option<String> = row.get(3).unwrap_or(None);
                    let deadline_time: Option<String> = row.get(4).unwrap_or(None);
                    let reminders_json: Option<String> = row.get(5).unwrap_or(None);
                    let Some(reminders) = parse_reminders(reminders_json.as_deref())
                    else {
                        continue;
                    };
                    let Some(due) = task_due_time(
                        scheduled_date.as_deref(),
                        deadline_date.as_deref(),
                        deadline_time.as_deref(),
                    ) else {
                        continue;
                    };
                    for r in &reminders {
                        if let Some(at) = trigger_time_for(&r.kind, due) {
                            if at >= earliest && at <= latest {
                                acc.push(Trigger {
                                    item_id: id.clone(),
                                    item_kind: ItemKind::Task,
                                    title: title.clone(),
                                    body: format_task_body(&due),
                                    trigger_at: at,
                                });
                            }
                        }
                    }
                }
            }
        }
        acc
    }

    /// Build the list of pending (not-yet-fired, in-the-future or
    /// just-overdue) reminder triggers for the scheduler. Limited to
    /// `MAX_HORIZON_DAYS` of lookahead so we don't fan out on long
    /// recurring series, with a small `GRACE_MINUTES` tolerance for
    /// just-overdue triggers we still want to deliver.
    fn collect_pending_triggers(&self) -> Vec<Trigger> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::minutes(GRACE_MINUTES);
        let latest = now + ChronoDuration::days(MAX_HORIZON_DAYS);
        let mut out = self.collect_triggers_in_window(earliest, latest);

        // Filter out anything we already fired in this process.
        let fired = self.fired.lock().expect("fired set poisoned");
        out.retain(|t| {
            !fired.contains(&FiredKey {
                item_id: t.item_id.clone(),
                trigger_iso: t.trigger_at.to_rfc3339(),
            })
        });
        out
    }

    fn fire<R: Runtime>(&self, app: &AppHandle<R>, t: &Trigger) {
        info!(
            item_kind = ?t.item_kind,
            item_id = %t.item_id,
            "firing reminder"
        );
        let result = app
            .notification()
            .builder()
            .title(&t.title)
            .body(&t.body)
            .show();
        if let Err(err) = result {
            warn!(?err, "failed to dispatch notification");
        }
        let mut fired = self.fired.lock().expect("fired set poisoned");
        fired.insert(FiredKey {
            item_id: t.item_id.clone(),
            trigger_iso: t.trigger_at.to_rfc3339(),
        });
    }

    /// Look for `app_start` reminders whose due time has already passed
    /// and fire them at startup. Conceptually a one-shot version of the
    /// main loop limited to that reminder type.
    fn fire_app_start_reminders<R: Runtime>(&self, app: &AppHandle<R>) {
        let now = Utc::now();
        let to_fire: Vec<Trigger> = {
            let conn = match self.db.lock() {
                Ok(c) => c,
                Err(err) => {
                    warn!(?err, "reminder DB mutex poisoned");
                    return;
                }
            };
            let mut acc: Vec<Trigger> = Vec::new();

            // Events
            if let Ok(mut stmt) = conn.prepare(EVENT_QUERY) {
                if let Ok(mut rows) = stmt.query(params![]) {
                    while let Some(row) = rows.next().unwrap_or(None) {
                        let id: String = row.get(0).unwrap_or_default();
                        let title: String = row.get(1).unwrap_or_default();
                        let start_str: String = row.get(2).unwrap_or_default();
                        let reminders_json: Option<String> = row.get(3).unwrap_or(None);
                        let Some(reminders) = parse_reminders(reminders_json.as_deref())
                        else {
                            continue;
                        };
                        let Ok(start) = start_str.parse::<DateTime<Utc>>() else {
                            continue;
                        };
                        for r in &reminders {
                            if matches!(r.kind, ReminderKind::AppStart) && start <= now {
                                acc.push(Trigger {
                                    item_id: id.clone(),
                                    item_kind: ItemKind::Event,
                                    title: title.clone(),
                                    body: format_event_body(&start),
                                    trigger_at: now,
                                });
                            }
                        }
                    }
                }
            }

            // Tasks
            if let Ok(mut stmt) = conn.prepare(TASK_QUERY) {
                if let Ok(mut rows) = stmt.query(params![]) {
                    while let Some(row) = rows.next().unwrap_or(None) {
                        let id: String = row.get(0).unwrap_or_default();
                        let title: String = row.get(1).unwrap_or_default();
                        let scheduled_date: Option<String> = row.get(2).unwrap_or(None);
                        let deadline_date: Option<String> = row.get(3).unwrap_or(None);
                        let deadline_time: Option<String> = row.get(4).unwrap_or(None);
                        let reminders_json: Option<String> = row.get(5).unwrap_or(None);
                        let Some(reminders) = parse_reminders(reminders_json.as_deref())
                        else {
                            continue;
                        };
                        let Some(due) = task_due_time(
                            scheduled_date.as_deref(),
                            deadline_date.as_deref(),
                            deadline_time.as_deref(),
                        ) else {
                            continue;
                        };
                        for r in &reminders {
                            if matches!(r.kind, ReminderKind::AppStart) && due <= now {
                                acc.push(Trigger {
                                    item_id: id.clone(),
                                    item_kind: ItemKind::Task,
                                    title: title.clone(),
                                    body: format_task_body(&due),
                                    trigger_at: now,
                                });
                            }
                        }
                    }
                }
            }
            acc
        };

        // Lock dropped here; safe to dispatch notifications.
        for t in to_fire {
            self.fire(app, &t);
        }
    }
}

/// How many days into the future the scheduler looks ahead in a single
/// pass. A new mutation invalidates the loop, so the horizon only
/// needs to be long enough to comfortably bridge a quiet stretch.
const MAX_HORIZON_DAYS: i64 = 30;
/// Trigger times within this many minutes in the past still fire.
/// Above that we treat them as "missed" and skip — usually the
/// app_start logic will pick them up instead.
const GRACE_MINUTES: i64 = 5;
/// How far back the Ctrl+Shift+R overview looks for already-passed
/// reminders. A week of backlog is enough for "did I miss something
/// yesterday?" without flooding the list.
const OVERVIEW_PAST_DAYS: i64 = 7;
/// How far forward the overview shows upcoming reminders. Longer
/// than the scheduler horizon so the user can plan ahead.
const OVERVIEW_FUTURE_DAYS: i64 = 90;

/// SELECT id, title, start_utc, reminders FROM events
const EVENT_QUERY: &str = "SELECT id, title, start_utc, reminders FROM events";

/// SELECT id, title, scheduled_date, deadline_date, deadline_time, reminders FROM tasks
const TASK_QUERY: &str =
    "SELECT id, title, scheduled_date, deadline_date, deadline_time, reminders FROM tasks";

fn trigger_time_for(
    kind: &ReminderKind,
    reference: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    match kind {
        ReminderKind::Relative { minutes_before } => {
            Some(reference - ChronoDuration::minutes(*minutes_before))
        }
        ReminderKind::Absolute { at } => Some(*at),
        ReminderKind::AppStart => None, // handled separately at startup
        ReminderKind::Email { .. } => None, // adapter-side, not local
    }
}

fn parse_reminders(json: Option<&str>) -> Option<Vec<Reminder>> {
    let raw = json?;
    if raw.is_empty() {
        return None;
    }
    match serde_json::from_str::<Vec<Reminder>>(raw) {
        Ok(list) if !list.is_empty() => Some(list),
        _ => None,
    }
}

fn task_due_time(
    scheduled: Option<&str>,
    deadline: Option<&str>,
    time: Option<&str>,
) -> Option<DateTime<Utc>> {
    let date = scheduled.or(deadline)?;
    // Task dates are stored as YYYY-MM-DD; combine with optional time
    // or default to 09:00 local. We parse in the local timezone via
    // chrono::Local and then convert to UTC.
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
    let nd = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let nt = if let Some(t) = time {
        NaiveTime::parse_from_str(t, "%H:%M:%S")
            .or_else(|_| NaiveTime::parse_from_str(t, "%H:%M"))
            .ok()?
    } else {
        NaiveTime::from_hms_opt(9, 0, 0)?
    };
    let local = chrono::Local
        .from_local_datetime(&NaiveDateTime::new(nd, nt))
        .single()?;
    Some(local.with_timezone(&Utc))
}

fn format_event_body(start: &DateTime<Utc>) -> String {
    use chrono::TimeZone;
    let local = chrono::Local.from_utc_datetime(&start.naive_utc());
    local.format("%H:%M").to_string()
}

fn format_task_body(due: &DateTime<Utc>) -> String {
    use chrono::TimeZone;
    let local = chrono::Local.from_utc_datetime(&due.naive_utc());
    local.format("%Y-%m-%d %H:%M").to_string()
}

/// Thin alias for the shared scheduler handle that command modules
/// pull out of `tauri::State`.
pub type SchedulerHandle = Arc<ReminderScheduler>;
