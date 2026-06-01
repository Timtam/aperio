//! Reminder scheduler (DESIGN.md section 14).
//!
//! Single background tokio task that
//!   1. enumerates the upcoming reminder triggers across every event
//!      and task that the app can see — both the local SQLite store
//!      and every registered external adapter (iCloud, Google,
//!      Microsoft Graph, EWS, iCal feeds);
//!   2. sleeps until the next one fires (or until something signals
//!      that the schedule has changed);
//!   3. dispatches an OS notification via `tauri-plugin-notification`
//!      and records the fire so a restart within the same minute does
//!      not double-notify.
//!
//! Re-computation is signalled through a [`tokio::sync::Notify`]. The
//! CRUD command layer calls [`ReminderScheduler::invalidate`] after
//! any event or task mutation; the worker wakes up, throws away its
//! pending wait, and re-scans. Local triggers are read directly from
//! SQLite on every scan (fast). External-adapter triggers are kept in
//! a TTL cache (see `EXTERNAL_TRIGGERS_TTL`) so a flurry of local
//! mutations doesn't slam every registered server with full
//! list-and-fetch round-trips.
//!
//! Per-calendar default reminders (Settings → Kalender, mirrored from
//! iOS's "Standard-Hinweise") are overlaid here too: if an external
//! event arrives with `reminders: []`, the calendar's stored default
//! list is substituted. Same idea the EventDialog uses on the JS side,
//! re-implemented backend-side because the notification scheduler
//! never goes through the JS overlay path.
//!
//! Storage of "already fired" reminders is in-memory only. A crash
//! between a fire and the next scan can re-deliver a reminder; the
//! sync wave (Phase 7) will persist it.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cal_adapter_local::SharedConn;
use cal_core::{
    DateRange, Event, EventRecurrence, RecurrenceEnd, RecurrenceFrequency, Reminder, ReminderKind,
    Task, TaskRecurrence,
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, NaiveTime, TimeZone, Utc};
use rrule::{RRule, RRuleSet, Tz as RruleTz};
use rusqlite::params;
use serde::Serialize;
use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::registry::AdapterRegistry;
use crate::user_prefs::UserPrefsRepo;

/// Identifier for a single fired reminder. Two reminders fire at the
/// "same" trigger only if they share item id, kind, AND timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FiredKey {
    item_id: String,
    trigger_iso: String,
}

/// One concrete reminder occurrence, after expanding recurrence and
/// resolving relative offsets to absolute UTC times.
///
/// `relevant_until` is the wall-clock time after which firing this
/// reminder no longer makes sense — set to the event's end for
/// timed events, the all-day window's end for all-day events, or
/// the task's due time. The scheduler's catch-up logic uses it to
/// decide whether a past-trigger reminder should still fire when
/// the app starts after the trigger time but before the event
/// itself has happened.
#[derive(Debug, Clone)]
struct Trigger {
    item_id: String,
    item_kind: ItemKind,
    title: String,
    body: String,
    trigger_at: DateTime<Utc>,
    relevant_until: DateTime<Utc>,
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

/// Cache slot for the external-adapter scan. The TTL keeps the
/// scheduler from refetching iCloud / Graph / EWS on every local
/// `invalidate()` call — most mutations don't change external state
/// at all, and the small `EXTERNAL_TRIGGERS_TTL` window means a real
/// remote change shows up within a few minutes either way.
struct ExternalTriggerCache {
    fetched_at: Instant,
    triggers: Vec<Trigger>,
}

pub struct ReminderScheduler {
    db: SharedConn,
    registry: Arc<AdapterRegistry>,
    invalidate: Arc<Notify>,
    fired: Arc<Mutex<HashSet<FiredKey>>>,
    external_cache: Arc<Mutex<Option<ExternalTriggerCache>>>,
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
    pub fn spawn<R: Runtime>(
        db: SharedConn,
        registry: Arc<AdapterRegistry>,
        app: AppHandle<R>,
    ) -> Arc<Self> {
        let scheduler = Arc::new(Self {
            db,
            registry,
            invalidate: Arc::new(Notify::new()),
            fired: Arc::new(Mutex::new(HashSet::new())),
            external_cache: Arc::new(Mutex::new(None)),
        });
        let worker = scheduler.clone();
        tauri::async_runtime::spawn(async move {
            // First sweep covers the "fired while we were offline"
            // case for app_start reminders. Local-only for now —
            // external adapters don't carry the AppStart kind on the
            // wire.
            worker.fire_app_start_reminders(&app);
            worker.worker_loop(app).await;
        });
        scheduler
    }

    /// Wake the worker so it re-scans. Safe to call from any thread;
    /// multiple back-to-back calls are coalesced by `Notify`. Local
    /// triggers re-scan on every wake; external triggers come from
    /// the TTL cache unless the caller also calls
    /// `invalidate_external_cache`.
    pub fn invalidate(&self) {
        self.invalidate.notify_one();
    }

    /// Drop the external-trigger snapshot so the next scan re-fans
    /// out to every adapter. Used by settings flows whose change
    /// affects how external events resolve to Triggers — most
    /// notably "Standard-Hinweise" for a calendar (per-calendar
    /// default reminders), where the cached Triggers were materialised
    /// against the OLD defaults and won't reflect the new ones until
    /// the cache expires.
    ///
    /// Pairs with `invalidate()`: caller usually wants both — clear
    /// the cache, then wake the worker so it picks up the change
    /// without waiting for the next polling tick.
    pub fn invalidate_external_cache(&self) {
        let mut guard = self.external_cache.lock().expect("external cache poison");
        *guard = None;
    }

    /// Snapshot reminder triggers for the Ctrl+Shift+R overview
    /// dialog. Includes both already-passed and upcoming triggers
    /// within a generous window so the user can review what fired
    /// recently and what is still pending. Sorted ascending by
    /// trigger time and capped at `limit` to keep the dialog snappy.
    pub async fn upcoming(&self, limit: usize) -> Vec<UpcomingReminder> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::days(OVERVIEW_PAST_DAYS);
        let latest = now + ChronoDuration::days(OVERVIEW_FUTURE_DAYS);
        let mut triggers = self.collect_triggers_in_window(earliest, latest).await;
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
            let triggers = self.collect_pending_triggers().await;
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
                    // Nothing scheduled within the horizon. Block on
                    // an invalidate, but with a periodic fallback so
                    // a freshly-added external reminder that lands
                    // after the cache TTL still gets discovered.
                    tokio::select! {
                        _ = self.invalidate.notified() => continue,
                        _ = tokio::time::sleep(EMPTY_HORIZON_RETRY) => continue,
                    }
                }
            }
        }
    }

    /// Walk every source (local SQLite + each external adapter) once
    /// and collect every reminder trigger whose absolute trigger
    /// time falls inside `[earliest, latest]`. No filtering for
    /// "already fired" — callers layer that on top.
    async fn collect_triggers_in_window(
        &self,
        earliest: DateTime<Utc>,
        latest: DateTime<Utc>,
    ) -> Vec<Trigger> {
        let mut acc = self.collect_local_triggers_in_window(earliest, latest);
        let external = self.external_triggers_cached_or_fetch().await;
        for t in external {
            if t.trigger_at >= earliest && t.trigger_at <= latest {
                acc.push(t);
            }
        }
        acc
    }

    /// Local-only collector. Fast (single SQLite scan) — called on
    /// every wake regardless of cache state.
    fn collect_local_triggers_in_window(
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

        // Events — RRULE-expanded the same way the external adapter
        // path is, so a recurring local event fires a reminder for
        // each occurrence inside the window instead of only the
        // series master. `end_utc` flows into the per-occurrence
        // `relevant_until` so the catch-up logic in
        // `collect_pending_triggers` can decide whether a
        // just-missed reminder is still worth firing.
        if let Ok(mut stmt) = conn.prepare(EVENT_QUERY) {
            if let Ok(mut rows) = stmt.query(params![]) {
                while let Some(row) = rows.next().unwrap_or(None) {
                    let id: String = row.get(0).unwrap_or_default();
                    let title: String = row.get(1).unwrap_or_default();
                    let start_str: String = row.get(2).unwrap_or_default();
                    let end_str: String = row.get(3).unwrap_or_default();
                    let reminders_json: Option<String> = row.get(4).unwrap_or(None);
                    let rrule: Option<String> = row.get(5).unwrap_or(None);
                    let exceptions_json: Option<String> = row.get(6).unwrap_or(None);
                    let Some(reminders) = parse_reminders(reminders_json.as_deref()) else {
                        continue;
                    };
                    let Ok(start) = start_str.parse::<DateTime<Utc>>() else {
                        continue;
                    };
                    let end = end_str.parse::<DateTime<Utc>>().unwrap_or(start);
                    let duration = (end - start).max(ChronoDuration::zero());
                    let recurrence = rrule.map(|rule| EventRecurrence {
                        rrule: rule,
                        exceptions: parse_rrule_exceptions(exceptions_json.as_deref()),
                    });
                    acc.extend(occurrence_triggers(
                        &id,
                        ItemKind::Event,
                        &title,
                        start,
                        recurrence.as_ref(),
                        &reminders,
                        duration,
                        earliest,
                        latest,
                    ));
                }
            }
        }

        // Tasks — recurrence-aware via the same `occurrence_triggers`
        // primitive the events path uses. `task_recurrence_to_rrule_body`
        // turns the structured `TaskRecurrence` into an RFC 5545
        // RRULE body the expansion helper understands, so a weekly
        // chore fires reminders for every week within the horizon.
        if let Ok(mut stmt) = conn.prepare(TASK_QUERY) {
            if let Ok(mut rows) = stmt.query(params![]) {
                while let Some(row) = rows.next().unwrap_or(None) {
                    let id: String = row.get(0).unwrap_or_default();
                    let title: String = row.get(1).unwrap_or_default();
                    let scheduled_date: Option<String> = row.get(2).unwrap_or(None);
                    let scheduled_time: Option<String> = row.get(3).unwrap_or(None);
                    let deadline_date: Option<String> = row.get(4).unwrap_or(None);
                    let deadline_time: Option<String> = row.get(5).unwrap_or(None);
                    let reminders_json: Option<String> = row.get(6).unwrap_or(None);
                    let recurrence_json: Option<String> = row.get(7).unwrap_or(None);

                    let Some(reminders) = parse_reminders(reminders_json.as_deref()) else {
                        continue;
                    };

                    // Parse the date/time pair into the structured
                    // types `master_due` understands so the local
                    // and external paths agree on the master due
                    // (scheduled-time wins over deadline-time when
                    // scheduled-date won the date arbitration).
                    let sd = scheduled_date
                        .as_deref()
                        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
                    let st = scheduled_time.as_deref().and_then(parse_local_time);
                    let dd = deadline_date
                        .as_deref()
                        .and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
                    let dt = deadline_time.as_deref().and_then(parse_local_time);
                    let Some(due) = master_due(sd, st, dd, dt) else {
                        continue;
                    };

                    let recurrence = recurrence_json
                        .as_deref()
                        .and_then(|s| serde_json::from_str::<TaskRecurrence>(s).ok())
                        .and_then(|rec| {
                            task_recurrence_to_rrule_body(&rec).map(|rrule| EventRecurrence {
                                rrule,
                                exceptions: Vec::new(),
                            })
                        });

                    acc.extend(occurrence_triggers(
                        &id,
                        ItemKind::Task,
                        &title,
                        due,
                        recurrence.as_ref(),
                        &reminders,
                        // See `task_triggers`: tasks are point-in-
                        // time, relevant_until == due, catch-up
                        // fires for not-yet-overdue rows only.
                        ChronoDuration::zero(),
                        earliest,
                        latest,
                    ));
                }
            }
        }
        acc
    }

    /// Read the external-trigger cache, or refresh it if absent /
    /// stale. The cache holds the *full* `now ± EXTERNAL_HORIZON`
    /// window so the caller can slice by any sub-window without
    /// triggering another fetch.
    async fn external_triggers_cached_or_fetch(&self) -> Vec<Trigger> {
        {
            let guard = self.external_cache.lock().expect("external cache poison");
            if let Some(cached) = guard.as_ref() {
                if cached.fetched_at.elapsed() < EXTERNAL_TRIGGERS_TTL {
                    return cached.triggers.clone();
                }
            }
        }
        let fresh = self.fetch_external_triggers().await;
        let mut guard = self.external_cache.lock().expect("external cache poison");
        *guard = Some(ExternalTriggerCache {
            fetched_at: Instant::now(),
            triggers: fresh.clone(),
        });
        fresh
    }

    /// Fan out across every registered external adapter and pull a
    /// snapshot of every event + task reminder trigger within the
    /// scheduler horizon. Errors per calendar / list don't poison
    /// the rest — they're logged and the other rows still come
    /// through.
    async fn fetch_external_triggers(&self) -> Vec<Trigger> {
        let now = Utc::now();
        let from = now - ChronoDuration::days(EXTERNAL_PAST_DAYS);
        let to = now + ChronoDuration::days(EXTERNAL_FUTURE_DAYS);
        // The CalDAV `get_events` filter takes a DateRange in event-
        // start coordinates. For recurring events we expand
        // occurrences within the padded window below, but iCloud only
        // returns the master if its own DTSTART falls inside this
        // range — so we widen here too. A two-week pad either side
        // covers Apple's longest reminder preset and matches
        // `EVENT_EXPANSION_PAD`.
        let (occ_from, occ_to) = occurrence_window(from, to);
        let range = DateRange::new(occ_from, occ_to);

        let mut acc: Vec<Trigger> = Vec::new();

        // ── Calendars → events ────────────────────────────────────
        for (account_id, adapter) in self.registry.snapshot_calendar_adapters() {
            let calendars = match adapter.list_calendars().await {
                Ok(c) => c,
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "list_calendars failed during reminder scan",
                    );
                    continue;
                }
            };
            for cal in calendars {
                let defaults = self.calendar_default_reminders(&cal.id);
                let events = match adapter.get_events(&cal.id, range).await {
                    Ok(e) => e,
                    Err(err) => {
                        warn!(
                            account_id = %account_id,
                            calendar_id = %cal.id,
                            ?err,
                            "get_events failed during reminder scan",
                        );
                        continue;
                    }
                };
                acc.extend(event_triggers(&events, &defaults, from, to));
            }
        }

        // ── Task lists → tasks ────────────────────────────────────
        for (account_id, adapter) in self.registry.snapshot_task_adapters() {
            let lists = match adapter.list_task_lists().await {
                Ok(l) => l,
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "list_task_lists failed during reminder scan",
                    );
                    continue;
                }
            };
            for list in lists {
                let tasks = match adapter.get_tasks(&list.id).await {
                    Ok(t) => t,
                    Err(err) => {
                        warn!(
                            account_id = %account_id,
                            list_id = %list.id,
                            ?err,
                            "get_tasks failed during reminder scan",
                        );
                        continue;
                    }
                };
                acc.extend(task_triggers(&tasks, from, to));
            }
        }

        acc
    }

    /// Look up the user's "Settings → Kalender" default reminders for
    /// `calendar_id`. Same key the frontend hook
    /// `useCalendarDefaultReminders` writes to. Empty when nothing has
    /// been configured — the wire reminders win as-is in that case.
    fn calendar_default_reminders(&self, calendar_id: &str) -> Vec<Reminder> {
        let key = format!("calendar.{}.defaultReminders", calendar_id);
        let repo = UserPrefsRepo::new(&self.db);
        let Ok(Some(raw)) = repo.get(&key) else {
            return Vec::new();
        };
        serde_json::from_str::<Vec<Reminder>>(&raw).unwrap_or_default()
    }

    /// Build the list of pending (not-yet-fired, in-the-future or
    /// catch-up-eligible) reminder triggers for the scheduler.
    /// Lookahead is bounded by `MAX_HORIZON_DAYS` to keep RRULE
    /// expansion costs sane on long recurring series.
    ///
    /// Catch-up window: the past bound reaches `CATCH_UP_HORIZON`
    /// days back so reminders that lapsed while the app was closed
    /// (or before an external adapter finished its first fetch)
    /// still make it into the scan. The relevance filter below
    /// drops the ones whose event has already happened — a
    /// reminder for tomorrow's standup that "fired" three hours ago
    /// is still useful; one for last Friday's lunch isn't.
    async fn collect_pending_triggers(&self) -> Vec<Trigger> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::days(CATCH_UP_HORIZON);
        let latest = now + ChronoDuration::days(MAX_HORIZON_DAYS);
        let mut out = self.collect_triggers_in_window(earliest, latest).await;

        // De-duplicate (same item_id + same trigger time appearing
        // from both local SQLite and an external adapter is unlikely
        // but possible — keep the first occurrence).
        let mut seen: HashSet<(String, String)> = HashSet::new();
        out.retain(|t| seen.insert((t.item_id.clone(), t.trigger_at.to_rfc3339())));

        // Catch-up filter. Future triggers always stay (the worker
        // schedules them via tokio::sleep). Past triggers stay only
        // when the event itself is still relevant — see
        // `catchup_eligible` for the rule.
        out.retain(|t| catchup_eligible(t, now, CATCH_UP_GRACE));

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

    /// Look for `app_start` reminders whose due time has already
    /// passed and fire them at startup. Local-only on purpose —
    /// `ReminderKind::AppStart` is an Aperio-local concept; no wire
    /// format carries it. CalDAV / Graph / EWS round-trips
    /// intentionally drop AppStart kinds (see
    /// `cal-adapter-caldav::mapping::reminder_to_alarm`).
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

            // Events. The same EVENT_QUERY shape feeds the regular
            // scan and the AppStart catch-up; column indices are
            // now id(0), title(1), start_utc(2), end_utc(3),
            // reminders(4), rrule(5), rrule_exceptions(6). AppStart
            // doesn't care about recurrence or duration, so we read
            // only the columns we need.
            if let Ok(mut stmt) = conn.prepare(EVENT_QUERY) {
                if let Ok(mut rows) = stmt.query(params![]) {
                    while let Some(row) = rows.next().unwrap_or(None) {
                        let id: String = row.get(0).unwrap_or_default();
                        let title: String = row.get(1).unwrap_or_default();
                        let start_str: String = row.get(2).unwrap_or_default();
                        let reminders_json: Option<String> = row.get(4).unwrap_or(None);
                        let Some(reminders) = parse_reminders(reminders_json.as_deref()) else {
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
                                    // AppStart fires at app start;
                                    // the event itself is the
                                    // reference, so relevant_until
                                    // matches the (already-passed)
                                    // start. The dedupe pass
                                    // ignores this field, and the
                                    // catch-up filter doesn't run
                                    // on this code path (we
                                    // explicitly want to fire here).
                                    relevant_until: start,
                                });
                            }
                        }
                    }
                }
            }

            // Tasks. AppStart fires once per process for the master
            // due time — recurrence is irrelevant here (the kind
            // means "as soon as the user opens the app after the
            // reference time"), so we keep the master-only behaviour
            // even though the surrounding scanner is now
            // recurrence-aware. Column indices follow the new
            // TASK_QUERY shape (scheduled_time added at index 3,
            // recurrence at index 7).
            if let Ok(mut stmt) = conn.prepare(TASK_QUERY) {
                if let Ok(mut rows) = stmt.query(params![]) {
                    while let Some(row) = rows.next().unwrap_or(None) {
                        let id: String = row.get(0).unwrap_or_default();
                        let title: String = row.get(1).unwrap_or_default();
                        let scheduled_date: Option<String> = row.get(2).unwrap_or(None);
                        let scheduled_time: Option<String> = row.get(3).unwrap_or(None);
                        let deadline_date: Option<String> = row.get(4).unwrap_or(None);
                        let deadline_time: Option<String> = row.get(5).unwrap_or(None);
                        let reminders_json: Option<String> = row.get(6).unwrap_or(None);
                        let Some(reminders) = parse_reminders(reminders_json.as_deref()) else {
                            continue;
                        };
                        // Mirror the new scanner: pair each date
                        // with its own time-of-day rather than
                        // collapsing both into deadline_time.
                        let date = scheduled_date.as_deref().or(deadline_date.as_deref());
                        let time = if scheduled_date.is_some() {
                            scheduled_time.as_deref()
                        } else {
                            deadline_time.as_deref()
                        };
                        let Some(due) = task_due_time(date, None, time) else {
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
                                    relevant_until: due,
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
/// How far back the scheduler reaches when looking for catch-up
/// candidates — reminders whose trigger time lapsed while the app
/// was closed or before an external adapter's first fetch
/// returned. The relevance filter
/// (`trigger_at > now || relevant_until > now`) drops anything
/// where the event itself is already over, so this bound is just a
/// scan-size guard rather than the policy bound. Seven days keeps
/// the worst-case "I haven't opened the app in a week" within
/// reach while still bounded for the cache layer.
const CATCH_UP_HORIZON: i64 = 7;
/// A small grace added to `relevant_until` when deciding whether a
/// past-trigger reminder is still worth firing. Lets a reminder
/// for an event that started a couple of minutes ago still ring
/// when the app opens — useful for the "I'm walking into the
/// meeting" case where the user joined late.
const CATCH_UP_GRACE: ChronoDuration = ChronoDuration::minutes(5);
/// How far back the Ctrl+Shift+R overview looks for already-passed
/// reminders. A week of backlog is enough for "did I miss something
/// yesterday?" without flooding the list.
const OVERVIEW_PAST_DAYS: i64 = 7;
/// How far forward the overview shows upcoming reminders. Longer
/// than the scheduler horizon so the user can plan ahead.
const OVERVIEW_FUTURE_DAYS: i64 = 90;
/// Window the external-adapter fan-out fetches per pass. Slightly
/// wider than the overview's forward window so a single fetch can
/// serve both the scheduler and the overview without re-running.
const EXTERNAL_PAST_DAYS: i64 = 7;
const EXTERNAL_FUTURE_DAYS: i64 = 90;
/// Lifetime of the external-trigger snapshot. Five minutes matches
/// the CalDAV listing cache — both windows are short enough that a
/// freshly created reminder still becomes eligible to fire within
/// the time the user expects "I just added a reminder" to mean.
const EXTERNAL_TRIGGERS_TTL: Duration = Duration::from_secs(5 * 60);
/// When the local + external scan returns nothing, the worker still
/// wakes up periodically so a freshly-added external reminder that
/// landed AFTER the cache went stale still gets picked up. Matches
/// the cache TTL so the next loop runs with a fresh external snapshot.
const EMPTY_HORIZON_RETRY: Duration = EXTERNAL_TRIGGERS_TTL;

/// SELECT id, title, start_utc, end_utc, reminders, rrule, rrule_exceptions FROM events
///
/// `end_utc` joins the others so the scheduler can compute each
/// event's duration — used by the catch-up filter so a reminder
/// whose fire time lapsed still goes out when the app starts
/// before the event itself has happened (or is still in progress).
///
/// `rrule` + `rrule_exceptions` let the scheduler expand recurring
/// events into per-occurrence reminders instead of seeing only the
/// master's start. Non-recurring events still work — both columns
/// come back NULL and the expansion helper degrades to a
/// single-occurrence vector containing the master start.
const EVENT_QUERY: &str = "SELECT id, title, start_utc, end_utc, reminders, \
    rrule, rrule_exceptions FROM events";

/// SELECT id, title, scheduled_date, scheduled_time, deadline_date,
/// deadline_time, reminders, recurrence FROM tasks
///
/// The extra columns make recurring tasks reach the scheduler the
/// same way recurring events do — `recurrence` is the JSON-serialised
/// `TaskRecurrence` (or NULL for non-recurring rows), and
/// `scheduled_time` lets us pair the date with its own time-of-day
/// (the original query collapsed scheduled and deadline times into
/// the deadline_time column, which lost precision when both were
/// set).
const TASK_QUERY: &str = "SELECT id, title, \
    scheduled_date, scheduled_time, \
    deadline_date, deadline_time, \
    reminders, recurrence FROM tasks";

/// Translate a batch of external events into Trigger entries. Empty
/// `reminders` on an event falls back to the calendar's stored default
/// reminder list (mirrors iOS's "Default Alert Times" behaviour — the
/// VALARM isn't on the wire, the user wants it applied locally).
///
/// Recurring events (`event.recurrence: Some(...)`) get expanded
/// inside `[window_start, window_end]` so every occurrence whose
/// reminder fires in the scheduler horizon emits its own trigger,
/// not just the series master. EXDATE exceptions from
/// `EventRecurrence.exceptions` are honoured; the rest of the
/// occurrence list flows back through the same trigger emission
/// loop a non-recurring event would.
fn event_triggers(
    events: &[Event],
    calendar_defaults: &[Reminder],
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Vec<Trigger> {
    let mut out = Vec::new();
    for ev in events {
        let effective: &[Reminder] = if ev.reminders.is_empty() {
            calendar_defaults
        } else {
            &ev.reminders
        };
        if effective.is_empty() {
            continue;
        }
        // Event-shaped duration: the wall-clock span the event
        // occupies. Drives the catch-up logic — a reminder whose
        // trigger lapsed but whose event is still in progress (or
        // hasn't started yet) is still useful on app start; one for
        // an event already ended isn't.
        let duration = (ev.end - ev.start).max(ChronoDuration::zero());
        out.extend(occurrence_triggers(
            &ev.id,
            ItemKind::Event,
            &ev.title,
            ev.start,
            ev.recurrence.as_ref(),
            effective,
            duration,
            window_start,
            window_end,
        ));
    }
    out
}

/// The single primitive every event-trigger emission path funnels
/// through. Given an item's master start + (optional) recurrence
/// spec + reminders + occurrence duration, produce every Trigger
/// whose fire time falls inside `[window_start, window_end]`.
///
/// Recurrence handling: when `recurrence` is `Some`, the master's
/// `start` becomes the DTSTART for an RRULE expansion bounded by a
/// padded version of the window (see `event_expansion_window`).
/// Without recurrence the function emits the single master start.
/// Either way, each occurrence + each reminder produces one Trigger.
///
/// `duration` is the offset added to each occurrence start to
/// compute its `relevant_until` — events pass `event.end -
/// event.start`, tasks pass `Duration::zero()` (a task's "relevant
/// until" is the due time itself).
fn occurrence_triggers(
    item_id: &str,
    item_kind: ItemKind,
    title: &str,
    master_start: DateTime<Utc>,
    recurrence: Option<&EventRecurrence>,
    reminders: &[Reminder],
    duration: ChronoDuration,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Vec<Trigger> {
    let starts: Vec<DateTime<Utc>> = match recurrence {
        Some(rec) => expand_occurrences(
            master_start,
            &rec.rrule,
            &rec.exceptions,
            window_start,
            window_end,
        ),
        None => vec![master_start],
    };

    let mut out = Vec::with_capacity(starts.len() * reminders.len());
    for occ_start in starts {
        let relevant_until = occ_start + duration;
        for r in reminders {
            let Some(at) = trigger_time_for(&r.kind, occ_start) else {
                continue;
            };
            if at < window_start || at > window_end {
                // Bound the emission to the requested window so a
                // weekly series doesn't fill `out` with months of
                // out-of-range triggers — the caller filters again,
                // but doing it here keeps the cache + sort small.
                continue;
            }
            out.push(Trigger {
                item_id: item_id.to_string(),
                item_kind,
                title: title.to_string(),
                body: format_event_body(&occ_start),
                trigger_at: at,
                relevant_until,
            });
        }
    }
    out
}

/// Expand an RRULE into the occurrence list within `[start_bound,
/// end_bound]`. EXDATEs from the master are honoured. Robustness
/// notes:
///
///   - `start_bound`/`end_bound` here are the OCCURRENCE-start
///     bounds, not the reminder-fire bounds. Callers pad the
///     reminder window outward by `EVENT_EXPANSION_PAD` to cover
///     the maximum sensible reminder lead time (one week).
///   - A bad / unparseable RRULE degrades to the single master
///     start so the user still gets a reminder for the first
///     occurrence — same fall-through the JS expansion uses.
///   - `RRULESET_LIMIT` caps unbounded series (e.g. "weekly
///     forever") so a runaway rule can't allocate gigabytes.
fn expand_occurrences(
    dt_start_utc: DateTime<Utc>,
    rrule_body: &str,
    exceptions: &[DateTime<Utc>],
    start_bound: DateTime<Utc>,
    end_bound: DateTime<Utc>,
) -> Vec<DateTime<Utc>> {
    let trimmed = rrule_body.trim();
    let body = trimmed.strip_prefix("RRULE:").unwrap_or(trimmed);

    let unvalidated: RRule<rrule::Unvalidated> = match body.parse() {
        Ok(r) => r,
        Err(err) => {
            warn!(
                rrule = %body,
                ?err,
                "failed to parse RRULE during reminder expansion; falling back to master start",
            );
            return vec![dt_start_utc];
        }
    };

    let dt_start = dt_start_utc.with_timezone(&RruleTz::UTC);
    let validated = match unvalidated.validate(dt_start) {
        Ok(v) => v,
        Err(err) => {
            warn!(
                rrule = %body,
                ?err,
                "RRULE failed validation; falling back to master start",
            );
            return vec![dt_start_utc];
        }
    };

    let mut set = RRuleSet::new(dt_start).rrule(validated);
    for ex in exceptions {
        set = set.exdate(ex.with_timezone(&RruleTz::UTC));
    }
    set = set
        .after(start_bound.with_timezone(&RruleTz::UTC))
        .before(end_bound.with_timezone(&RruleTz::UTC));

    let result = set.all(RRULESET_LIMIT);
    if result.limited {
        warn!(
            rrule = %body,
            limit = RRULESET_LIMIT,
            "RRULE expansion hit the scheduler safety limit",
        );
    }
    result
        .dates
        .into_iter()
        .map(|dt| dt.with_timezone(&Utc))
        .collect()
}

/// Hard cap on the number of occurrences we'll materialise from a
/// single RRULE — protects against unbounded rules like "every
/// minute forever" pinned to a stale start date. 500 covers a
/// 10-year weekly series, well above anything a calendar UI emits
/// in practice.
const RRULESET_LIMIT: u16 = 500;

/// Buffer added around the reminder window when expanding RRULE
/// occurrences. A reminder fires `δ` BEFORE its event; an event
/// starting at `window_end + δ` still has its reminder firing
/// inside the window. The buffer needs to cover the largest
/// reasonable `δ`. One week is generous — Apple's longest preset
/// is two weeks, but most relative reminders sit in the
/// minutes-to-hours range. A tighter bound would silently miss
/// long-lead reminders; a wider one just costs a few extra
/// occurrences to iterate.
const EVENT_EXPANSION_PAD: ChronoDuration = ChronoDuration::days(14);

/// Convenience: derive the occurrence-expansion bounds from the
/// reminder-fire window. Same value used by every collect path so
/// recurring events stay in sync between local SQL and external
/// adapter scans.
fn occurrence_window(
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> (DateTime<Utc>, DateTime<Utc>) {
    (
        window_start - EVENT_EXPANSION_PAD,
        window_end + EVENT_EXPANSION_PAD,
    )
}

/// Translate a batch of external tasks into Trigger entries. Same
/// scheduled-or-deadline reference-time resolution as the local-side
/// `task_due_time`, but operating on the structured `NaiveDate` /
/// `NaiveTime` fields directly so we don't round-trip through the
/// stringified columns the local SQLite path uses.
///
/// Recurring tasks (`task.recurrence: Some(...)`) get expanded the
/// same way recurring events do: the master's due time is the
/// DTSTART, the structured TaskRecurrence is translated to an RFC
/// 5545 RRULE body, `expand_occurrences` produces every due time
/// inside the reminder window. A task without recurrence still
/// emits exactly one set of triggers off its master due time.
fn task_triggers(
    tasks: &[Task],
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Vec<Trigger> {
    let mut out = Vec::new();
    for t in tasks {
        if t.reminders.is_empty() {
            continue;
        }
        let Some(master_due) = task_master_due(t) else {
            continue;
        };
        // Tasks use the same `EventRecurrence` shape as the
        // expansion helper expects — wrap the structured
        // TaskRecurrence into an RRULE body so a single set of
        // primitives serves both kinds of recurring rows.
        let recurrence = t.recurrence.as_ref().and_then(|rec| {
            task_recurrence_to_rrule_body(rec).map(|rrule| EventRecurrence {
                rrule,
                exceptions: Vec::new(),
            })
        });
        out.extend(occurrence_triggers(
            &t.id,
            ItemKind::Task,
            &t.title,
            master_due,
            recurrence.as_ref(),
            &t.reminders,
            // Tasks have no "duration" — they're a point in time,
            // not a span. relevant_until == due time, which means
            // catch-up fires only for not-yet-overdue tasks.
            ChronoDuration::zero(),
            window_start,
            window_end,
        ));
    }
    out
}

/// Master "due time" for a task — first occurrence's reference for
/// reminder triggers. Scheduled wins over deadline, mirroring the
/// `task_due_time` convention used everywhere else. Returns `None`
/// when neither date is set (purely backlogged tasks).
fn task_master_due(t: &Task) -> Option<DateTime<Utc>> {
    master_due(
        t.scheduled_date,
        t.scheduled_time,
        t.deadline_date,
        t.deadline_time,
    )
}

/// Resolution helper shared between the external `Task` path and the
/// local SQL scanner. Pairs each date with its OWN time-of-day —
/// `scheduled_date` uses `scheduled_time`, `deadline_date` uses
/// `deadline_time` — and falls back to 09:00 local when no time is
/// set, matching the convention `task_due_time` uses for the
/// stringified version.
fn master_due(
    scheduled_date: Option<chrono::NaiveDate>,
    scheduled_time: Option<NaiveTime>,
    deadline_date: Option<chrono::NaiveDate>,
    deadline_time: Option<NaiveTime>,
) -> Option<DateTime<Utc>> {
    let (date, time) = if let Some(d) = scheduled_date {
        (d, scheduled_time)
    } else if let Some(d) = deadline_date {
        (d, deadline_time)
    } else {
        return None;
    };
    let nt = time.unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).expect("9:00 is valid"));
    let local = chrono::Local
        .from_local_datetime(&NaiveDateTime::new(date, nt))
        .single()?;
    Some(local.with_timezone(&Utc))
}

/// Translate Aperio's structured `TaskRecurrence` into an RFC 5545
/// RRULE body that `expand_occurrences` can drive through the same
/// rrule crate the event path uses.
///
/// `None` when the recurrence is incomplete enough that no
/// occurrences would expand — e.g. a `RecurrenceEnd::After { 0 }`
/// or interval 0. Same defensive bailing the JS-side
/// TaskRecurrenceSelector applies before showing the rule.
///
/// Frequency / interval map 1:1. `day_of_week` becomes BYDAY,
/// `day_of_month` becomes BYMONTHDAY. `end`:
///
///   - `Never`           → no UNTIL/COUNT (rrule defaults to
///                         RRULESET_LIMIT-bounded "infinite")
///   - `After { n }`     → `COUNT=n`
///   - `OnDate { date }` → `UNTIL=YYYYMMDDT235959Z`. The end-of-day
///                         UTC bound matches how the JS selector
///                         interprets the picker — the user picks a
///                         date, and the rule is inclusive of that
///                         day's occurrences.
fn task_recurrence_to_rrule_body(rec: &TaskRecurrence) -> Option<String> {
    if rec.interval == 0 {
        return None;
    }
    let freq = match rec.frequency {
        RecurrenceFrequency::Daily => "DAILY",
        RecurrenceFrequency::Weekly => "WEEKLY",
        RecurrenceFrequency::Monthly => "MONTHLY",
        RecurrenceFrequency::Yearly => "YEARLY",
    };
    let mut parts: Vec<String> = vec![format!("FREQ={freq}")];
    if rec.interval > 1 {
        parts.push(format!("INTERVAL={}", rec.interval));
    }
    if let Some(days) = &rec.day_of_week {
        if !days.is_empty() {
            let by_day: Vec<&'static str> = days.iter().map(weekday_to_byday).collect();
            parts.push(format!("BYDAY={}", by_day.join(",")));
        }
    }
    if let Some(dom) = rec.day_of_month {
        if (1..=31).contains(&dom) {
            parts.push(format!("BYMONTHDAY={}", dom));
        }
    }
    match &rec.end {
        Some(RecurrenceEnd::After { occurrences }) => {
            if *occurrences == 0 {
                return None;
            }
            parts.push(format!("COUNT={}", occurrences));
        }
        Some(RecurrenceEnd::OnDate { date }) => {
            // RFC 5545 UNTIL must carry a UTC indicator for
            // DTSTART-with-time series. We use the same trick the
            // CalDAV adapter uses elsewhere — anchor at end-of-day
            // UTC so the picked date is inclusive.
            parts.push(format!("UNTIL={}T235959Z", date.format("%Y%m%d"),));
        }
        Some(RecurrenceEnd::Never) | None => {}
    }
    Some(parts.join(";"))
}

/// Map Aperio's weekday enum into RFC 5545 BYDAY two-letter codes.
fn weekday_to_byday(d: &cal_core::Weekday) -> &'static str {
    use cal_core::Weekday::*;
    match d {
        Monday => "MO",
        Tuesday => "TU",
        Wednesday => "WE",
        Thursday => "TH",
        Friday => "FR",
        Saturday => "SA",
        Sunday => "SU",
    }
}

/// Decide whether a Trigger should fire at `now`. Future triggers
/// always pass (the worker schedules them via `tokio::sleep`); past
/// triggers pass only when the underlying event is still relevant
/// — `relevant_until + grace` lets a reminder for an event that
/// just barely started still ring, useful for the "I'm walking
/// into the meeting" case.
///
/// Pure helper so the filter is unit-testable without spinning up
/// a scheduler instance.
fn catchup_eligible(t: &Trigger, now: DateTime<Utc>, grace: ChronoDuration) -> bool {
    t.trigger_at > now || t.relevant_until + grace > now
}

fn trigger_time_for(kind: &ReminderKind, reference: DateTime<Utc>) -> Option<DateTime<Utc>> {
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

/// Parse the `events.rrule_exceptions` JSON column into a list of
/// UTC EXDATE timestamps. Empty / NULL / unparseable falls through
/// to an empty Vec — the caller still expands the rest of the rule
/// normally, so a malformed exceptions field never blocks all
/// reminders on the row.
fn parse_rrule_exceptions(json: Option<&str>) -> Vec<DateTime<Utc>> {
    let Some(raw) = json else {
        return Vec::new();
    };
    if raw.is_empty() {
        return Vec::new();
    }
    serde_json::from_str::<Vec<DateTime<Utc>>>(raw).unwrap_or_default()
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
    use chrono::NaiveDate;
    let nd = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let nt = time
        .and_then(parse_local_time)
        .unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).expect("9:00 is valid"));
    let local = chrono::Local
        .from_local_datetime(&NaiveDateTime::new(nd, nt))
        .single()?;
    Some(local.with_timezone(&Utc))
}

/// Parse the `HH:MM[:SS]` strings the local SQLite stores into a
/// `NaiveTime`. Returns `None` for empty / unparseable values so the
/// caller can fall back to the 09:00-local default.
fn parse_local_time(raw: &str) -> Option<NaiveTime> {
    if raw.is_empty() {
        return None;
    }
    NaiveTime::parse_from_str(raw, "%H:%M:%S")
        .or_else(|_| NaiveTime::parse_from_str(raw, "%H:%M"))
        .ok()
}

fn format_event_body(start: &DateTime<Utc>) -> String {
    let local = chrono::Local.from_utc_datetime(&start.naive_utc());
    local.format("%H:%M").to_string()
}

fn format_task_body(due: &DateTime<Utc>) -> String {
    let local = chrono::Local.from_utc_datetime(&due.naive_utc());
    local.format("%Y-%m-%d %H:%M").to_string()
}

/// Thin alias for the shared scheduler handle that command modules
/// pull out of `tauri::State`.
pub type SchedulerHandle = Arc<ReminderScheduler>;

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::{EventRecurrence, Reminder, ReminderKind, Task, TaskStatus};
    use chrono::{NaiveDate, NaiveTime};

    fn rel(minutes_before: i64) -> Reminder {
        Reminder {
            kind: ReminderKind::Relative { minutes_before },
            sound: None,
        }
    }

    fn make_event(reminders: Vec<Reminder>) -> Event {
        Event {
            id: "ev-1".into(),
            calendar_id: "cal-1".into(),
            title: "Meeting".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 5, 20, 8, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
            all_day: false,
            recurrence: None as Option<EventRecurrence>,
            color_label: None,
            reminders,
            sound: None,
            attendees: Vec::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
        }
    }

    fn make_task(
        scheduled_date: Option<NaiveDate>,
        scheduled_time: Option<NaiveTime>,
        deadline_date: Option<NaiveDate>,
        deadline_time: Option<NaiveTime>,
        reminders: Vec<Reminder>,
    ) -> Task {
        Task {
            assignees: Vec::new(),
            id: "task-1".into(),
            list_id: "list-1".into(),
            title: "Write report".into(),
            description: None,
            status: TaskStatus::Open,
            priority: cal_core::TaskPriority::Medium,
            scheduled_date,
            scheduled_time,
            deadline_date,
            deadline_time,
            recurrence: None,
            parent_id: None,
            section_id: None,
            color_label: None,
            reminders,
            sound: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            completed_at: None,
            etag: None,
        }
    }

    /// Generous test window that comfortably contains every
    /// fixture's start date — `event_triggers` filters to it, so
    /// tests that don't care about windowing use this as a no-op
    /// pair.
    fn wide_window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2027, 1, 1, 0, 0, 0).unwrap(),
        )
    }

    #[test]
    fn event_with_explicit_reminder_uses_it_and_ignores_calendar_default() {
        // The event carries its own VALARM-equivalent. Calendar
        // defaults stay defaults — they only matter when the event's
        // reminder list is empty.
        let ev = make_event(vec![rel(15)]);
        let (ws, we) = wide_window();
        let triggers = event_triggers(&[ev], &[rel(60)], ws, we);
        assert_eq!(triggers.len(), 1);
        // 8:00 start − 15 min = 7:45.
        assert_eq!(
            triggers[0].trigger_at,
            Utc.with_ymd_and_hms(2026, 5, 20, 7, 45, 0).unwrap(),
        );
    }

    #[test]
    fn event_with_empty_reminders_falls_back_to_calendar_default() {
        // The exact iCloud "Standard-Hinweis" case: the event came
        // back with no VALARM, the calendar has a default configured
        // in Settings → Kalender, the trigger fires from the
        // default.
        let ev = make_event(Vec::new());
        let (ws, we) = wide_window();
        let triggers = event_triggers(&[ev], &[rel(15)], ws, we);
        assert_eq!(triggers.len(), 1);
        assert_eq!(
            triggers[0].trigger_at,
            Utc.with_ymd_and_hms(2026, 5, 20, 7, 45, 0).unwrap(),
        );
    }

    #[test]
    fn event_with_no_reminders_and_no_default_emits_nothing() {
        let ev = make_event(Vec::new());
        let (ws, we) = wide_window();
        let triggers = event_triggers(&[ev], &[], ws, we);
        assert!(triggers.is_empty());
    }

    #[test]
    fn event_with_multiple_defaults_emits_one_trigger_each() {
        let ev = make_event(Vec::new());
        let (ws, we) = wide_window();
        let triggers = event_triggers(&[ev], &[rel(60), rel(10)], ws, we);
        assert_eq!(triggers.len(), 2);
    }

    fn make_recurring_event(rrule: &str, exceptions: Vec<DateTime<Utc>>) -> Event {
        let mut ev = make_event(vec![rel(15)]);
        ev.recurrence = Some(EventRecurrence {
            rrule: rrule.to_string(),
            exceptions,
        });
        ev
    }

    #[test]
    fn recurring_event_emits_a_trigger_per_occurrence_in_window() {
        // Weekly Wednesday meeting (DTSTART 2026-05-20 is also a
        // Wednesday). Within a four-week window we expect four
        // occurrences and therefore four triggers — one per week,
        // each firing 15 minutes before the 08:00 start.
        let ev = make_recurring_event("FREQ=WEEKLY;BYDAY=WE", Vec::new());
        let triggers = event_triggers(
            &[ev],
            &[],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 17, 0, 0, 0).unwrap(),
        );
        assert_eq!(triggers.len(), 4, "expected 4 weekly occurrences");
        // First occurrence's reminder: 2026-05-20 07:45.
        assert_eq!(
            triggers[0].trigger_at,
            Utc.with_ymd_and_hms(2026, 5, 20, 7, 45, 0).unwrap(),
        );
        // Last occurrence's reminder: 2026-06-10 07:45.
        assert_eq!(
            triggers[3].trigger_at,
            Utc.with_ymd_and_hms(2026, 6, 10, 7, 45, 0).unwrap(),
        );
    }

    #[test]
    fn recurring_event_honours_exdate_exceptions() {
        // Same weekly series, but the user excluded the 2026-05-27
        // occurrence (e.g. via the "delete only this occurrence"
        // flow). Triggers for that date must NOT appear.
        let ev = make_recurring_event(
            "FREQ=WEEKLY;BYDAY=WE",
            vec![Utc.with_ymd_and_hms(2026, 5, 27, 8, 0, 0).unwrap()],
        );
        let triggers = event_triggers(
            &[ev],
            &[],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap(),
        );
        // Master + week 3 expected (week 2 skipped).
        let dates: Vec<_> = triggers.iter().map(|t| t.trigger_at.date_naive()).collect();
        assert!(dates.contains(&chrono::NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()));
        assert!(!dates.contains(&chrono::NaiveDate::from_ymd_opt(2026, 5, 27).unwrap()));
        assert!(dates.contains(&chrono::NaiveDate::from_ymd_opt(2026, 6, 3).unwrap()));
    }

    #[test]
    fn recurring_event_falls_back_to_master_on_bad_rrule() {
        // Garbage RRULE — the expansion helper warn-logs and
        // degrades to the master start. The first occurrence's
        // reminder still fires; subsequent ones simply don't. Better
        // than dropping the row entirely.
        let ev = make_recurring_event("BOGUS=NOPE", Vec::new());
        let triggers = event_triggers(
            &[ev],
            &[],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 4, 0, 0, 0).unwrap(),
        );
        assert_eq!(triggers.len(), 1);
        assert_eq!(
            triggers[0].trigger_at,
            Utc.with_ymd_and_hms(2026, 5, 20, 7, 45, 0).unwrap(),
        );
    }

    #[test]
    fn recurring_event_window_filter_excludes_out_of_range_occurrences() {
        // Weekly series, but the window is "next two weeks only".
        // Occurrences outside that window must NOT emit triggers
        // even if the RRULE rolls them out — keeps cache sizes
        // bounded for daily or hourly rules.
        let ev = make_recurring_event("FREQ=WEEKLY;BYDAY=WE", Vec::new());
        let triggers = event_triggers(
            &[ev],
            &[],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 3, 0, 0, 0).unwrap(),
        );
        assert_eq!(triggers.len(), 2, "only the first two weekly occurrences");
    }

    fn task_window() -> (DateTime<Utc>, DateTime<Utc>) {
        (
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        )
    }

    #[test]
    fn task_triggers_prefer_scheduled_over_deadline_date() {
        // Both scheduled and deadline are set; the reference time
        // comes from scheduled — same precedence the local
        // `task_due_time` uses so the two code paths stay
        // consistent.
        let task = make_task(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            Some(NaiveTime::from_hms_opt(10, 0, 0).unwrap()),
            Some(NaiveDate::from_ymd_opt(2026, 5, 25).unwrap()),
            Some(NaiveTime::from_hms_opt(18, 0, 0).unwrap()),
            vec![rel(0)], // fire at the reference time itself
        );
        let (ws, we) = task_window();
        let triggers = task_triggers(&[task], ws, we);
        assert_eq!(triggers.len(), 1);
        // 10:00 local = depends on zone; assert just the date so the
        // test is portable. The original date wins (scheduled), not
        // 2026-05-25 (deadline).
        let dt_local = chrono::Local.from_utc_datetime(&triggers[0].trigger_at.naive_utc());
        assert_eq!(
            dt_local.date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()
        );
    }

    #[test]
    fn task_triggers_default_to_nine_am_when_time_missing() {
        // No time-of-day → 09:00 local, same convention as the local
        // SQLite path.
        let task = make_task(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            None,
            None,
            None,
            vec![rel(0)],
        );
        let (ws, we) = task_window();
        let triggers = task_triggers(&[task], ws, we);
        assert_eq!(triggers.len(), 1);
        let dt_local = chrono::Local.from_utc_datetime(&triggers[0].trigger_at.naive_utc());
        assert_eq!(dt_local.time(), NaiveTime::from_hms_opt(9, 0, 0).unwrap());
    }

    #[test]
    fn task_without_any_date_emits_nothing() {
        let task = make_task(None, None, None, None, vec![rel(15)]);
        let (ws, we) = task_window();
        assert!(task_triggers(&[task], ws, we).is_empty());
    }

    #[test]
    fn task_with_no_reminders_emits_nothing() {
        let task = make_task(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            None,
            None,
            None,
            Vec::new(),
        );
        let (ws, we) = task_window();
        assert!(task_triggers(&[task], ws, we).is_empty());
    }

    // ── TaskRecurrence → RRULE conversion ────────────────────────

    fn weekday(d: cal_core::Weekday) -> Vec<cal_core::Weekday> {
        vec![d]
    }

    #[test]
    fn task_recurrence_weekly_with_interval_and_byday_serialises() {
        // Every two weeks on Wed & Fri, ends after 4 occurrences.
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 2,
            day_of_week: Some(vec![
                cal_core::Weekday::Wednesday,
                cal_core::Weekday::Friday,
            ]),
            day_of_month: None,
            end: Some(RecurrenceEnd::After { occurrences: 4 }),
        };
        let body = task_recurrence_to_rrule_body(&rec).expect("valid rrule");
        // The exact ordering of parts is deterministic in the
        // helper; assert each fragment is present to keep the test
        // tolerant if we later reorder for readability.
        assert!(body.contains("FREQ=WEEKLY"), "got: {body}");
        assert!(body.contains("INTERVAL=2"), "got: {body}");
        assert!(body.contains("BYDAY=WE,FR"), "got: {body}");
        assert!(body.contains("COUNT=4"), "got: {body}");
    }

    #[test]
    fn task_recurrence_monthly_on_day_of_month_until_date() {
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Monthly,
            interval: 1,
            day_of_week: None,
            day_of_month: Some(15),
            end: Some(RecurrenceEnd::OnDate {
                date: NaiveDate::from_ymd_opt(2026, 12, 31).unwrap(),
            }),
        };
        let body = task_recurrence_to_rrule_body(&rec).expect("valid rrule");
        assert!(body.contains("FREQ=MONTHLY"), "got: {body}");
        // Interval omitted when 1 — keeps the rule compact.
        assert!(!body.contains("INTERVAL"), "got: {body}");
        assert!(body.contains("BYMONTHDAY=15"), "got: {body}");
        assert!(body.contains("UNTIL=20261231T235959Z"), "got: {body}");
    }

    #[test]
    fn task_recurrence_with_zero_interval_rejects() {
        // Defensive: a corrupt persisted value with interval=0 would
        // produce an infinite loop in some RRULE implementations.
        // We reject it before handing to the expansion helper.
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 0,
            day_of_week: None,
            day_of_month: None,
            end: None,
        };
        assert!(task_recurrence_to_rrule_body(&rec).is_none());
    }

    #[test]
    fn task_recurrence_count_zero_rejects() {
        // After-zero-occurrences is a degenerate rule (no triggers
        // would ever fire). Bail rather than emit `COUNT=0` which
        // the rrule crate would reject in validation anyway.
        let rec = TaskRecurrence {
            frequency: RecurrenceFrequency::Daily,
            interval: 1,
            day_of_week: None,
            day_of_month: None,
            end: Some(RecurrenceEnd::After { occurrences: 0 }),
        };
        assert!(task_recurrence_to_rrule_body(&rec).is_none());
    }

    // ── End-to-end recurring task triggers ──────────────────────

    fn make_recurring_task(rec: TaskRecurrence) -> Task {
        let mut t = make_task(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            Some(NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
            None,
            None,
            vec![rel(0)],
        );
        t.recurrence = Some(rec);
        t
    }

    #[test]
    fn recurring_task_emits_a_trigger_per_occurrence_in_window() {
        // Weekly on Wednesdays (2026-05-20 IS a Wednesday).
        // 4-week window → 4 triggers.
        let task = make_recurring_task(TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            day_of_week: Some(weekday(cal_core::Weekday::Wednesday)),
            day_of_month: None,
            end: None,
        });
        let triggers = task_triggers(
            &[task],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 6, 17, 0, 0, 0).unwrap(),
        );
        assert_eq!(
            triggers.len(),
            4,
            "expected four weekly Wednesday occurrences"
        );
    }

    #[test]
    fn recurring_task_after_n_occurrences_stops_at_n() {
        // After 2 occurrences: master + 1 more.
        let task = make_recurring_task(TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            day_of_week: Some(weekday(cal_core::Weekday::Wednesday)),
            day_of_month: None,
            end: Some(RecurrenceEnd::After { occurrences: 2 }),
        });
        let triggers = task_triggers(
            &[task],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            // Window wide enough to see four if the rule allowed.
            Utc.with_ymd_and_hms(2026, 6, 20, 0, 0, 0).unwrap(),
        );
        assert_eq!(triggers.len(), 2, "COUNT=2 should cap at two triggers");
    }

    // ── Catch-up filter ─────────────────────────────────────────

    fn future_trigger(at_offset_min: i64, relevant_offset_min: i64) -> Trigger {
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        Trigger {
            item_id: "t".into(),
            item_kind: ItemKind::Event,
            title: "T".into(),
            body: String::new(),
            trigger_at: now + ChronoDuration::minutes(at_offset_min),
            relevant_until: now + ChronoDuration::minutes(relevant_offset_min),
        }
    }

    #[test]
    fn catchup_keeps_future_triggers_unconditionally() {
        // The worker sleeps until future triggers fire; the filter
        // must not preemptively drop them.
        let t = future_trigger(30, 60);
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        assert!(catchup_eligible(&t, now, CATCH_UP_GRACE));
    }

    #[test]
    fn catchup_fires_recently_missed_when_event_not_started() {
        // The exact scenario the user reported: trigger 22 min ago,
        // event still 38 min in the future. The reminder must fire
        // immediately on the next scan.
        let t = future_trigger(-22, 38);
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        assert!(
            catchup_eligible(&t, now, CATCH_UP_GRACE),
            "missed reminder for an event 38 min away should still fire",
        );
    }

    #[test]
    fn catchup_drops_reminders_for_events_already_over() {
        // Trigger 90 min ago, event ended 30 min ago. No useful
        // notification any more — drop.
        let t = future_trigger(-90, -30);
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        assert!(
            !catchup_eligible(&t, now, CATCH_UP_GRACE),
            "reminder for a finished event should NOT fire",
        );
    }

    #[test]
    fn catchup_fires_during_grace_after_event_start() {
        // Event started 3 min ago (timed events have
        // relevant_until == start, so this is the
        // "walked-into-the-meeting" case). With CATCH_UP_GRACE=5
        // min, the reminder still fires.
        let t = future_trigger(-15, -3);
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        assert!(catchup_eligible(&t, now, CATCH_UP_GRACE));
    }

    #[test]
    fn catchup_drops_after_grace_window_passes() {
        // Same as above but the event started 10 min ago — past
        // the CATCH_UP_GRACE window. Drop.
        let t = future_trigger(-25, -10);
        let now = Utc.with_ymd_and_hms(2026, 5, 20, 12, 0, 0).unwrap();
        assert!(!catchup_eligible(&t, now, CATCH_UP_GRACE));
    }

    #[test]
    fn event_trigger_uses_event_end_as_relevance() {
        // A 1-hour timed event: master_start=08:00, end=09:00.
        // The emitted trigger should carry relevant_until=09:00,
        // not 08:00 — gives the catch-up logic a full hour of
        // grace during which a missed reminder still rings.
        let ev = make_event(vec![rel(15)]);
        let (ws, we) = wide_window();
        let triggers = event_triggers(&[ev], &[], ws, we);
        assert_eq!(triggers.len(), 1);
        assert_eq!(
            triggers[0].relevant_until,
            Utc.with_ymd_and_hms(2026, 5, 20, 9, 0, 0).unwrap(),
        );
    }

    #[test]
    fn task_trigger_uses_due_time_as_relevance() {
        // A task with reminder 15 min before 09:00 (the default-
        // when-no-time case): triggers at 08:45, relevant_until at
        // 09:00. After 09:00 the task is overdue and the catch-up
        // filter drops it.
        let task = make_task(
            Some(NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()),
            None,
            None,
            None,
            vec![rel(15)],
        );
        let (ws, we) = task_window();
        let triggers = task_triggers(&[task], ws, we);
        assert_eq!(triggers.len(), 1);
        // relevant_until == due time. Local-time 09:00 round-trips
        // through chrono::Local to UTC; assert the date-level fact.
        let due_local = chrono::Local.from_utc_datetime(&triggers[0].relevant_until.naive_utc());
        assert_eq!(due_local.time(), NaiveTime::from_hms_opt(9, 0, 0).unwrap(),);
    }

    #[test]
    fn recurring_task_until_date_excludes_later_occurrences() {
        // UNTIL bounds the rule — occurrences strictly after the
        // until date must not appear, even if the window extends
        // beyond it.
        let task = make_recurring_task(TaskRecurrence {
            frequency: RecurrenceFrequency::Weekly,
            interval: 1,
            day_of_week: Some(weekday(cal_core::Weekday::Wednesday)),
            day_of_month: None,
            end: Some(RecurrenceEnd::OnDate {
                date: NaiveDate::from_ymd_opt(2026, 6, 3).unwrap(),
            }),
        });
        let triggers = task_triggers(
            &[task],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        );
        // 2026-05-20, 05-27, 06-03 — 3 occurrences, 06-10 excluded.
        assert_eq!(triggers.len(), 3, "UNTIL=06-03 should leave three triggers");
    }
}
