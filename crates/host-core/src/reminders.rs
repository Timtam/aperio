//! Reminder trigger enumeration (DESIGN.md §14) — the Tauri-free core.
//!
//! Walks every reminder source (the local SQLite store + each registered
//! external adapter), expands recurrence, resolves relative offsets to absolute
//! UTC times, and produces a flat list of [`Trigger`]s within a window. The
//! §14.4 notification sound is resolved here too (via [`crate::sound`]), at
//! enumerate time.
//!
//! This is the shared half: the desktop `ReminderScheduler` (a Tauri async
//! worker that sleeps until the next trigger, fires an OS notification, plays
//! custom audio, and tracks already-fired triggers) calls these functions, and
//! so does the mobile cal-ffi reminder surface (which instead schedules the
//! triggers as ahead-of-time OS-delivered local notifications). One source of
//! truth for *what* fires *when*; each platform owns *how* it's delivered.

use std::collections::HashMap;
use std::sync::Arc;

use cal_core::{
    ContactsFeature, DateRange, Event, EventRecurrence, NewEvent, RecurrenceEnd,
    RecurrenceFrequency, Reminder, ReminderKind, SoundConfig, Task, TaskRecurrence, TaskUser,
};
use chrono::{DateTime, Duration as ChronoDuration, NaiveDateTime, NaiveTime, TimeZone, Utc};
use rrule::{RRule, RRuleSet, Tz as RruleTz};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::accounts::{AccountsRepo, AdapterKind};
use crate::cache::CacheStore;
use crate::db::SharedConn;
use crate::registry::AdapterRegistry;
use crate::sound::{ContainerKind, SoundPrefs};
use crate::user_prefs::UserPrefsRepo;

/// One concrete reminder occurrence, after expanding recurrence and resolving
/// relative offsets to absolute UTC times.
///
/// `relevant_until` is the wall-clock time after which firing this reminder no
/// longer makes sense — the event's end for timed events, the task's due time
/// otherwise. The desktop scheduler's catch-up logic uses it to decide whether
/// a past-trigger reminder should still fire at app start.
#[derive(Debug, Clone)]
pub struct Trigger {
    pub item_id: String,
    pub item_kind: ItemKind,
    /// The owning container — a task's `list_id`, an event's `calendar_id`. Lets
    /// a reminders UI route a tap on this row to the underlying item's editor.
    pub container_id: String,
    pub title: String,
    pub body: String,
    pub trigger_at: DateTime<Utc>,
    pub relevant_until: DateTime<Utc>,
    /// The occurrence's own start instant (NOT `trigger_at`, which for an all-day
    /// item is the day-carryover fire time). With `relevant_until` (= occurrence
    /// end) and `all_day`, lets a notification say "Ganztägig · 24. Juni bis 26.
    /// Juni" instead of a meaningless "00:00".
    pub start: DateTime<Utc>,
    /// The item is an all-day event (its reminders anchor to the day-carryover
    /// time). Tasks and timed events are `false`.
    pub all_day: bool,
    /// Effective notification sound, already resolved through the §14.4
    /// hierarchy (reminder → item → container → global → System).
    pub sound: SoundConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Event,
    Task,
}

/// Public DTO for a reminders overview / the mobile scheduler payload.
#[derive(Debug, Clone, Serialize)]
pub struct UpcomingReminder {
    pub item_id: String,
    pub item_kind: ItemKind,
    pub title: String,
    /// Trigger timestamp in RFC 3339 / ISO 8601 UTC.
    pub trigger_at: String,
}

/// A small grace added to `relevant_until` when deciding whether a past-trigger
/// reminder is still worth firing — lets a reminder for an event that started a
/// couple of minutes ago still ring. The desktop scheduler reads this when
/// filtering pending triggers; exposed so it and [`catchup_eligible`]'s tests
/// share one value.
pub const CATCH_UP_GRACE: ChronoDuration = ChronoDuration::minutes(5);

/// Window the external-adapter fan-out fetches per pass (now ± these). Slightly
/// wider than the desktop overview's forward window so one fetch serves both.
const EXTERNAL_PAST_DAYS: i64 = 7;
const EXTERNAL_FUTURE_DAYS: i64 = 90;

/// `SELECT id, title, start_utc, end_utc, reminders, rrule, rrule_exceptions,
/// calendar_id, all_day FROM events` — `end_utc` drives each event's duration
/// (the catch-up relevance window); `rrule`/`rrule_exceptions` drive
/// per-occurrence expansion; `calendar_id` resolves the §14.4 container sound;
/// `all_day` routes the reminder to the day-carryover anchor instead of a
/// minutes-before-midnight offset.
const EVENT_QUERY: &str = "SELECT id, title, start_utc, end_utc, reminders, \
    rrule, rrule_exceptions, calendar_id, all_day FROM events";

/// `SELECT id, title, scheduled_date, scheduled_time, deadline_date,
/// deadline_time, reminders, recurrence, list_id FROM tasks` — `recurrence` is
/// the JSON `TaskRecurrence`; `scheduled_time` pairs each date with its own
/// time-of-day; `list_id` resolves the §14.4 container sound.
const TASK_QUERY: &str = "SELECT id, title, \
    scheduled_date, scheduled_time, \
    deadline_date, deadline_time, \
    reminders, recurrence, list_id FROM tasks";

/// Local-only collector: every event + task reminder trigger from the local
/// SQLite store whose absolute trigger time falls inside `[earliest, latest]`.
/// No "already fired" filtering — callers layer that on. Loads the §14.4 sound
/// snapshot ONCE before locking the connection (the scan holds the lock for its
/// whole pass and `std::sync::Mutex` isn't reentrant).
pub fn enumerate_local_triggers(
    db: &SharedConn,
    earliest: DateTime<Utc>,
    latest: DateTime<Utc>,
) -> Vec<Trigger> {
    let sound_prefs = SoundPrefs::load(db);
    // Read the day-carryover anchor BEFORE locking the connection — the pref
    // read takes its own lock and `std::sync::Mutex` isn't reentrant.
    let day_start = day_start_time(db);
    // Local calendars have no adapter, so the external fan-out's overlay never
    // sees them: read their configured defaults here, BEFORE the scan locks the
    // connection (each pref read takes its own lock).
    let calendar_defaults = local_calendar_default_reminders(db);
    let conn = match db.lock() {
        Ok(c) => c,
        Err(err) => {
            warn!(?err, "reminder DB mutex poisoned");
            return Vec::new();
        }
    };
    let mut acc: Vec<Trigger> = Vec::new();

    // Events — RRULE-expanded so a recurring local event fires a reminder for
    // each occurrence in the window. `end_utc` flows into each occurrence's
    // `relevant_until` for the catch-up logic.
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
                let calendar_id: String = row.get(7).unwrap_or_default();
                let all_day: bool = row.get(8).unwrap_or(false);
                // The calendar's defaults ride along — the same merge rule
                // `event_triggers` applies to adapter-backed calendars.
                let reminders = effective_reminders(
                    &parse_reminders(reminders_json.as_deref()).unwrap_or_default(),
                    calendar_defaults
                        .get(&calendar_id)
                        .map(Vec::as_slice)
                        .unwrap_or(&[]),
                );
                if reminders.is_empty() {
                    continue;
                }
                let Ok(start) = start_str.parse::<DateTime<Utc>>() else {
                    continue;
                };
                let end = end_str.parse::<DateTime<Utc>>().unwrap_or(start);
                let duration = (end - start).max(ChronoDuration::zero());
                let recurrence = rrule.map(|rule| EventRecurrence {
                    rrule: rule,
                    exceptions: parse_rrule_exceptions(exceptions_json.as_deref()),
                    tzid: None,
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
                    &sound_prefs,
                    ContainerKind::Calendar,
                    &calendar_id,
                    all_day.then_some(day_start),
                ));
            }
        }
    }

    // Tasks — recurrence-aware via the same `occurrence_triggers` primitive.
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
                let list_id: String = row.get(8).unwrap_or_default();

                let Some(reminders) = parse_reminders(reminders_json.as_deref()) else {
                    continue;
                };

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
                            tzid: None,
                        })
                    });

                acc.extend(occurrence_triggers(
                    &id,
                    ItemKind::Task,
                    &title,
                    due,
                    recurrence.as_ref(),
                    &reminders,
                    ChronoDuration::zero(),
                    earliest,
                    latest,
                    &sound_prefs,
                    ContainerKind::TaskList,
                    &list_id,
                    None, // tasks are never all-day
                ));
            }
        }
    }
    acc
}

/// Mirror of the TS `isMineOrUnassigned` (shared/taskAssignment.ts): a task is
/// "mine to be reminded about" when the account has no identity (`me` is `None`
/// — local-style adapters), the task is unassigned, or I'm one of its assignees.
/// A colleague's task in a shared list (Vikunja/Todoist) is NOT, so it produces
/// no reminder — matching the day-start ownership filter and the calendar views.
fn is_mine_or_unassigned(assignees: &[TaskUser], me: Option<&TaskUser>) -> bool {
    match me {
        None => true,
        Some(me) => assignees.is_empty() || assignees.iter().any(|a| a.id == me.id),
    }
}

/// Fan out across every registered external adapter and pull a snapshot of
/// every event + task reminder trigger within the fixed `now ± EXTERNAL_*_DAYS`
/// horizon. Per-adapter errors are logged and skipped — one dead account never
/// blanks the rest. Events with no VALARM fall back to the calendar's stored
/// default reminders (the iOS "Default Alert Times" behaviour). Tasks assigned
/// only to OTHERS in a shared list are dropped (the day-start ownership rule).
pub async fn enumerate_external_triggers(
    registry: &Arc<AdapterRegistry>,
    db: &SharedConn,
) -> Vec<Trigger> {
    let now = Utc::now();
    let from = now - ChronoDuration::days(EXTERNAL_PAST_DAYS);
    let to = now + ChronoDuration::days(EXTERNAL_FUTURE_DAYS);
    // Recurring masters may sit outside the reminder window yet still have an
    // occurrence inside it — widen the fetch the same way the expansion does.
    let (occ_from, occ_to) = occurrence_window(from, to);
    let range = DateRange::new(occ_from, occ_to);

    // §14.4 sound snapshot. No connection lock is held on this async path, so
    // loading it here is deadlock-free.
    let sound_prefs = SoundPrefs::load(db);
    // Day-carryover anchor for all-day events (below).
    let day_start = day_start_time(db);

    // Device-local accounts (iOS EventKit / Android CalendarProvider) are
    // EXCLUDED from Aperio's reminder scheduling: the OS itself fires the alarms
    // on the device's own calendar + reminders, so scheduling Aperio
    // notifications for them — including via the calendar-default-reminder
    // fallback below — would double-notify. Every other external provider
    // (CalDAV / Graph / EWS / Google / Vikunja / Todoist) doesn't self-notify,
    // so it stays in.
    //
    // An unreadable account list leaves the set EMPTY, i.e. nothing suppressed.
    // That is the deliberate choice, not an oversight: the two directions trade
    // a duplicate alert against a missing one, and a missing appointment
    // reminder is the worse of the two. The scan re-runs on its own cadence, so
    // the duplicates last at most one pass. Logged so it isn't silent.
    let device_accounts: std::collections::HashSet<String> = match AccountsRepo::new(db).list() {
        Ok(accounts) => accounts
            .into_iter()
            .filter(|account| account.adapter_kind == AdapterKind::DEVICE_CALENDAR)
            .map(|account| account.id)
            .collect(),
        Err(err) => {
            warn!(
                ?err,
                "reminder scan: couldn't list accounts; device-local calendars may \
                 double-notify for this pass",
            );
            std::collections::HashSet::new()
        }
    };

    let mut acc: Vec<Trigger> = Vec::new();

    // ── Calendars → events ────────────────────────────────────────────────
    for (account_id, adapter) in registry.snapshot_calendar_adapters() {
        if device_accounts.contains(&account_id) {
            continue;
        }
        let calendars = match adapter.list_calendars().await {
            Ok(c) => c,
            Err(err) => {
                warn!(account_id = %account_id, ?err, "list_calendars failed during reminder scan");
                continue;
            }
        };
        for cal in calendars {
            let defaults = calendar_default_reminders(db, &cal.id);
            let events = match adapter.get_events(&cal.id, range).await {
                Ok(e) => e,
                Err(err) => {
                    warn!(account_id = %account_id, calendar_id = %cal.id, ?err, "get_events failed during reminder scan");
                    continue;
                }
            };
            acc.extend(event_triggers(
                &events,
                &defaults,
                &sound_prefs,
                from,
                to,
                day_start,
            ));
        }
    }

    // ── Task lists → tasks ────────────────────────────────────────────────
    for (account_id, adapter) in registry.snapshot_task_adapters() {
        if device_accounts.contains(&account_id) {
            continue;
        }
        // "me" for this account, so a shared list's tasks assigned only to
        // OTHERS produce no reminder. `None` for local-style adapters with no
        // identity → nothing is filtered. Fetched once per account; on the
        // desktop this whole pass is wrapped in the external TTL cache.
        let me = adapter.current_user().await.ok().flatten();
        let lists = match adapter.list_task_lists().await {
            Ok(l) => l,
            Err(err) => {
                warn!(account_id = %account_id, ?err, "list_task_lists failed during reminder scan");
                continue;
            }
        };
        for list in lists {
            let tasks = match adapter.get_tasks(&list.id).await {
                Ok(t) => t,
                Err(err) => {
                    warn!(account_id = %account_id, list_id = %list.id, ?err, "get_tasks failed during reminder scan");
                    continue;
                }
            };
            let mine: Vec<Task> = tasks
                .into_iter()
                .filter(|t| is_mine_or_unassigned(&t.assignees, me.as_ref()))
                .collect();
            acc.extend(task_triggers(&mine, &sound_prefs, from, to));
        }
    }

    acc
}

/// Birthday-calendar reminder triggers. The synthetic birthday calendars
/// (§10.3) aren't adapters, so [`enumerate_external_triggers`] never sees them —
/// left alone they'd fire nothing. This collector synthesises each birthday
/// calendar's all-day events (LOCAL contacts in-process, EXTERNAL from the
/// snapshot cache — never a network fetch) and applies that calendar's
/// configured DEFAULT reminders (`calendar.<id>.defaultReminders`, via
/// [`calendar_default_reminders`]).
///
/// Birthday events carry no per-event reminders of their own, so a birthday
/// calendar with no configured defaults fires nothing — the feature is opt-in
/// per calendar (e.g. the user adds "one week before"). Being all-day, each
/// reminder fires at the day-carryover time a whole number of days before the
/// birthday, exactly like a regular all-day event (see [`all_day_trigger_time`]).
pub async fn enumerate_birthday_triggers(
    local: &dyn ContactsFeature,
    registry: &Arc<AdapterRegistry>,
    cache: &Arc<CacheStore>,
    db: &SharedConn,
) -> Vec<Trigger> {
    let now = Utc::now();
    let from = now - ChronoDuration::days(EXTERNAL_PAST_DAYS);
    let to = now + ChronoDuration::days(EXTERNAL_FUTURE_DAYS);
    // Widen the synthesis window the same way the expansion does (a birthday
    // occurrence inside the window may sit just outside `[from, to]`).
    let (occ_from, occ_to) = occurrence_window(from, to);
    let range = DateRange::new(occ_from, occ_to);

    let sound_prefs = SoundPrefs::load(db);
    let day_start = day_start_time(db);

    let mut acc: Vec<Trigger> = Vec::new();
    for (cal, _account_id) in
        crate::birthdays::list_birthday_calendars(local, registry, cache).await
    {
        // Opt-OUT, not opt-in: a birthday calendar nobody has configured gets
        // the built-in default rather than silence. Only a list the user
        // deliberately emptied stays quiet.
        let defaults = configured_calendar_default_reminders(db, &cal.id)
            .unwrap_or_else(birthday_default_reminders);
        if defaults.is_empty() {
            continue;
        }
        let Some(events) =
            crate::birthdays::synthesise_birthday_events(local, registry, cache, &cal.id, range)
                .await
        else {
            continue;
        };
        acc.extend(event_triggers(
            &events,
            &defaults,
            &sound_prefs,
            from,
            to,
            day_start,
        ));
    }
    acc
}

/// Combined local + external enumeration within `[earliest, latest]`, for
/// callers without the desktop scheduler's external TTL cache (the mobile
/// host). External triggers use their own fixed horizon then get filtered to
/// the window. The desktop instead calls the local + external collectors
/// separately so it can wrap the external fetch in its TTL cache.
pub async fn enumerate_triggers(
    db: &SharedConn,
    registry: &Arc<AdapterRegistry>,
    earliest: DateTime<Utc>,
    latest: DateTime<Utc>,
) -> Vec<Trigger> {
    let mut acc = enumerate_local_triggers(db, earliest, latest);
    for t in enumerate_external_triggers(registry, db).await {
        if t.trigger_at >= earliest && t.trigger_at <= latest {
            acc.push(t);
        }
    }
    acc
}

/// `app_start` reminders (whose due time has already passed) from the LOCAL
/// store. Local-only by design — no wire format carries `ReminderKind::AppStart`
/// (CalDAV/Graph/EWS round-trips drop it). The caller fires these at startup;
/// recurrence is irrelevant (the kind means "as soon as the user opens the app
/// after the reference time").
pub fn enumerate_app_start_triggers(db: &SharedConn) -> Vec<Trigger> {
    let now = Utc::now();
    let sound_prefs = SoundPrefs::load(db);
    let conn = match db.lock() {
        Ok(c) => c,
        Err(err) => {
            warn!(?err, "reminder DB mutex poisoned");
            return Vec::new();
        }
    };
    let mut acc: Vec<Trigger> = Vec::new();

    if let Ok(mut stmt) = conn.prepare(EVENT_QUERY) {
        if let Ok(mut rows) = stmt.query(params![]) {
            while let Some(row) = rows.next().unwrap_or(None) {
                let id: String = row.get(0).unwrap_or_default();
                let title: String = row.get(1).unwrap_or_default();
                let start_str: String = row.get(2).unwrap_or_default();
                let reminders_json: Option<String> = row.get(4).unwrap_or(None);
                let calendar_id: String = row.get(7).unwrap_or_default();
                let all_day: bool = row.get(8).unwrap_or(false);
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
                            container_id: calendar_id.clone(),
                            title: title.clone(),
                            body: format_event_body(&start),
                            trigger_at: now,
                            relevant_until: start,
                            start,
                            all_day,
                            sound: sound_prefs.resolve(
                                r.sound.as_ref(),
                                &id,
                                ContainerKind::Calendar,
                                &calendar_id,
                            ),
                        });
                    }
                }
            }
        }
    }

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
                let list_id: String = row.get(8).unwrap_or_default();
                let Some(reminders) = parse_reminders(reminders_json.as_deref()) else {
                    continue;
                };
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
                            container_id: list_id.clone(),
                            title: title.clone(),
                            body: format_task_body(&due),
                            trigger_at: now,
                            relevant_until: due,
                            start: due,
                            all_day: false,
                            sound: sound_prefs.resolve(
                                r.sound.as_ref(),
                                &id,
                                ContainerKind::TaskList,
                                &list_id,
                            ),
                        });
                    }
                }
            }
        }
    }
    acc
}

/// Look up the user's "Settings → Kalender" default reminders for `calendar_id`
/// (the key `useCalendarDefaultReminders` writes). Empty when nothing is
/// configured — the wire reminders win as-is in that case.
///
/// A failed read also yields an empty list — there is nothing better to fall
/// back to, and the callers treat empty as "no defaults configured". It means a
/// configured default can go unapplied for one scan, so the failure is logged
/// rather than swallowed; the next pass picks the setting up again.
pub fn calendar_default_reminders(db: &SharedConn, calendar_id: &str) -> Vec<DefaultReminder> {
    configured_calendar_default_reminders(db, calendar_id).unwrap_or_default()
}

/// The configured default reminders of every calendar that has local events,
/// keyed by calendar id — only the calendars with a non-empty list. Read once
/// per scan, so the event loop can hold the connection lock without a pref
/// read underneath it (`std::sync::Mutex` isn't reentrant).
fn local_calendar_default_reminders(db: &SharedConn) -> HashMap<String, Vec<DefaultReminder>> {
    let ids: Vec<String> = match db.lock() {
        Ok(conn) => conn
            .prepare("SELECT DISTINCT calendar_id FROM events")
            .and_then(|mut stmt| {
                stmt.query_map([], |row| row.get::<_, String>(0))
                    .map(|rows| rows.flatten().collect::<Vec<String>>())
            })
            .unwrap_or_default(),
        Err(err) => {
            warn!(?err, "reminder DB mutex poisoned");
            return HashMap::new();
        }
    };
    ids.into_iter()
        .filter_map(|id| {
            let defaults = calendar_default_reminders(db, &id);
            (!defaults.is_empty()).then_some((id, defaults))
        })
        .collect()
}

/// The same read, but able to say "the user never answered this question".
///
/// `None` means no stored value at all; `Some(vec![])` means the list was
/// deliberately cleared. Collapsing those two into an empty vec is fine
/// wherever empty simply means "no defaults to apply" — but not where a
/// BUILT-IN default steps in, because there "never asked" must fire and
/// "switched off" must stay silent. See [`birthday_default_reminders`].
pub fn configured_calendar_default_reminders(
    db: &SharedConn,
    calendar_id: &str,
) -> Option<Vec<DefaultReminder>> {
    let key = format!("calendar.{}.defaultReminders", calendar_id);
    let repo = UserPrefsRepo::new(db);
    let raw = match repo.get(&key) {
        Ok(Some(raw)) => raw,
        Ok(None) => return None,
        Err(err) => {
            warn!(
                ?err,
                calendar_id = %calendar_id,
                "couldn't read the calendar's default reminders;                  treating this pass as if none were configured",
            );
            // Deliberately `Some(empty)`, not `None`: a failed READ must not
            // be mistaken for "never configured" and conjure a built-in
            // reminder the user may have switched off. Silence is the safe
            // answer to a broken read.
            return Some(Vec::new());
        }
    };
    Some(serde_json::from_str::<Vec<DefaultReminder>>(&raw).unwrap_or_default())
}

/// What a birthday calendar reminds you of when you have never said.
///
/// One reminder on the day itself. `minutes_before: 0` is read as whole DAYS
/// for an all-day event (see [`all_day_trigger_time`]), so it lands at the
/// user's own day-change time — never "two hours before", which for a date
/// carrying no time means nothing.
///
/// A birthday calendar exists BECAUSE somebody wants to be told, so silence
/// was the wrong default: it made the feature look broken to exactly the
/// person who asked for it. The editor still overrules this, including down
/// to nothing — a list cleared on purpose stays cleared.
pub fn birthday_default_reminders() -> Vec<DefaultReminder> {
    vec![DefaultReminder {
        reminder: Reminder {
            kind: ReminderKind::Relative { minutes_before: 0 },
            sound: None,
        },
        attach: false,
    }]
}

/// One entry of a calendar's default-reminder list: the reminder plus WHERE it
/// lives.
///
/// An entry that stays **in Aperio** is an overlay: the scheduler fires it for
/// every event of the calendar, in addition to the reminders the event carries
/// itself, and nothing is written into any event. An entry marked **attach**
/// is written into every NEW appointment created in the calendar as the
/// appointment's own reminder, so every other client of the calendar — the
/// iOS Calendar app, a voice assistant reading iCloud — rings too; for an
/// event that carries no reminders of its own (created elsewhere, or before
/// the choice existed) the scheduler still applies it here. That is what iOS
/// does with its own "Default Alert Times", and why an appointment created
/// there is announced everywhere while one created in Aperio used to be
/// silent outside Aperio.
///
/// Lists stored before the choice existed carry no `attach` field and read as
/// "in Aperio" — the behaviour they always had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefaultReminder {
    #[serde(flatten)]
    pub reminder: Reminder,
    /// Written into new appointments as their own reminder.
    #[serde(default)]
    pub attach: bool,
}

/// The entries that stay in Aperio, as plain reminders.
fn local_defaults(defaults: &[DefaultReminder]) -> impl Iterator<Item = &Reminder> {
    defaults.iter().filter(|d| !d.attach).map(|d| &d.reminder)
}

/// The entries written into new appointments, as plain reminders.
pub fn attached_defaults(defaults: &[DefaultReminder]) -> Vec<Reminder> {
    defaults
        .iter()
        .filter(|d| d.attach)
        .map(|d| d.reminder.clone())
        .collect()
}

/// What actually fires for one event of a calendar: the event's own reminders;
/// the attach entries when it has none of its own (they would be its own had
/// it been created here); and the in-Aperio entries always, on top — each
/// reminder at most once.
pub fn effective_reminders(own: &[Reminder], defaults: &[DefaultReminder]) -> Vec<Reminder> {
    let mut out: Vec<Reminder> = own.to_vec();
    if out.is_empty() {
        out.extend(attached_defaults(defaults));
    }
    for reminder in local_defaults(defaults) {
        if !out.contains(reminder) {
            out.push(reminder.clone());
        }
    }
    out
}

/// Write the calendar's attach-marked default reminders into a new
/// appointment. Returns whether the event was changed.
///
/// Applies only when the caller left the reminders UNSET (`reminders_unset`:
/// the editor was never touched and sent an empty list, or a quick-add that
/// has no reminder field at all) and the event carries none. An explicit
/// choice — including "no reminder at all" — is never overridden, and the
/// entries that stay in Aperio never touch the event: the scheduler overlays
/// them. Existing appointments are never touched by this: the update paths do
/// not call it, so an edit of an old event can't silently grow reminders.
pub fn apply_default_reminder_policy(
    db: &SharedConn,
    calendar_id: &str,
    event: &mut NewEvent,
    reminders_unset: bool,
) -> bool {
    if !reminders_unset || !event.reminders.is_empty() {
        return false;
    }
    let attached = attached_defaults(&calendar_default_reminders(db, calendar_id));
    if attached.is_empty() {
        return false;
    }
    event.reminders = attached;
    true
}

/// Translate a batch of external events into Trigger entries. The calendar's
/// stored defaults are folded in by [`effective_reminders`]: its in-Aperio
/// entries fire on top of whatever the event carries, and its attach entries
/// stand in when the event carries nothing (mirrors iOS's "Default Alert
/// Times" — the VALARM isn't on the wire, the user wants it applied locally).
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
    calendar_defaults: &[DefaultReminder],
    sound_prefs: &SoundPrefs,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    day_start: NaiveTime,
) -> Vec<Trigger> {
    let mut out = Vec::new();
    for ev in events {
        // A cancelled meeting never nags — skip it unconditionally, regardless
        // of the user's show-cancelled visibility setting (that only governs
        // rendering). EWS/CalDAV surface cancelled events as normal rows; Graph
        // keeps a cancelled whole-series/single. All of them land here.
        if ev.cancelled {
            continue;
        }
        let effective = effective_reminders(&ev.reminders, calendar_defaults);
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
            &effective,
            duration,
            window_start,
            window_end,
            sound_prefs,
            ContainerKind::Calendar,
            &ev.calendar_id,
            ev.all_day.then_some(day_start),
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
#[allow(clippy::too_many_arguments)]
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
    sound_prefs: &SoundPrefs,
    container_kind: ContainerKind,
    container_id: &str,
    // `Some(day_start_local)` for an ALL-DAY item — reminders fire a whole
    // number of days before the occurrence's date at this time-of-day (see
    // `all_day_trigger_time`). `None` for a timed item (the relative offset
    // subtracts from the start as before). Tasks always pass `None`.
    all_day_anchor: Option<NaiveTime>,
) -> Vec<Trigger> {
    let starts: Vec<DateTime<Utc>> = match recurrence {
        Some(rec) => expand_occurrences(
            master_start,
            &rec.rrule,
            &rec.exceptions,
            rec.tzid.as_deref(),
            window_start,
            window_end,
        ),
        None => vec![master_start],
    };

    let mut out = Vec::with_capacity(starts.len() * reminders.len());
    for occ_start in starts {
        let relevant_until = occ_start + duration;
        for r in reminders {
            let at = match all_day_anchor {
                Some(day_start) => all_day_trigger_time(occ_start, &r.kind, day_start),
                None => trigger_time_for(&r.kind, occ_start),
            };
            let Some(at) = at else {
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
                container_id: container_id.to_string(),
                title: title.to_string(),
                body: format_event_body(&occ_start),
                trigger_at: at,
                relevant_until,
                start: occ_start,
                all_day: all_day_anchor.is_some(),
                // §14.4: per-reminder override wins, else fall through
                // item → container → global → System.
                sound: sound_prefs.resolve(r.sound.as_ref(), item_id, container_kind, container_id),
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
///   - `tzid` is the zone the series was AUTHORED in, and it decides the
///     wall-clock the rule repeats at. Expanding in UTC instead is right only
///     for a floating or UTC series; for a zoned one the occurrences drift by
///     the DST offset the moment the series crosses a transition — an hour,
///     and near midnight a whole day. `cal_core::EventRecurrence::tzid`
///     documents exactly that, and this function used to ignore it.
fn expand_occurrences(
    dt_start_utc: DateTime<Utc>,
    rrule_body: &str,
    exceptions: &[DateTime<Utc>],
    tzid: Option<&str>,
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

    // The zone the rule repeats in. An unknown name degrades to UTC — the
    // previous behaviour for everything — rather than dropping the series.
    let zone = tzid
        .map(str::trim)
        .filter(|z| !z.is_empty())
        .and_then(|z| match z.parse::<chrono_tz::Tz>() {
            Ok(tz) => Some(RruleTz::Tz(tz)),
            Err(_) => {
                warn!(tzid = %z, "unknown TZID on a recurring event; expanding reminders in UTC");
                None
            }
        })
        .unwrap_or(RruleTz::UTC);
    // DTSTART carries the zone: rrule repeats at ITS wall clock, so a weekly
    // 09:00 series stays 09:00 across the DST boundary instead of sliding to
    // 08:00 or 10:00. The bounds and EXDATEs stay instants — they are compared,
    // not repeated, and an instant means the same moment in any zone.
    let dt_start = dt_start_utc.with_timezone(&zone);
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
    sound_prefs: &SoundPrefs,
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
                tzid: None,
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
            sound_prefs,
            ContainerKind::TaskList,
            &t.list_id,
            None, // tasks are never all-day
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
    // Scheduled day wins (carrying its time); else the deadline day; else no
    // anchor. A `match` rather than `if let … else if let … else return` so the
    // newer clippy's `question-mark` lint has nothing to fire on.
    let (date, time) = match (scheduled_date, deadline_date) {
        (Some(d), _) => (d, scheduled_time),
        (None, Some(d)) => (d, deadline_time),
        (None, None) => return None,
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
pub fn task_recurrence_to_rrule_body(rec: &TaskRecurrence) -> Option<String> {
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
pub fn catchup_eligible(t: &Trigger, now: DateTime<Utc>, grace: ChronoDuration) -> bool {
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

/// Fire time for a reminder on an ALL-DAY event (incl. synthesised birthdays).
///
/// A relative offset measured in MINUTES makes no sense against a midnight
/// start — "1 hour before" would ring at 23:00 the night before (the reported
/// bug). Instead we read the offset as whole DAYS and fire at the day-carryover
/// time-of-day (`tasks.dayStartTrigger`, passed in as `day_start`): "1 h" and
/// "on the day" both land on the event's own day at day-start, "1 day before"
/// the day before, "1 week before" seven days before. An explicit `Absolute`
/// reminder still wins. Days round to nearest (12 h → 1 day). The event's date
/// is its LOCAL date — every all-day producer (the editor, CalDAV, and now
/// birthdays via `birthdays::birthday_start_instant`) stores the start as
/// local-midnight-as-UTC, so this reads back as the intended day in any zone.
fn all_day_trigger_time(
    occ_start: DateTime<Utc>,
    kind: &ReminderKind,
    day_start: NaiveTime,
) -> Option<DateTime<Utc>> {
    let minutes_before = match kind {
        ReminderKind::Relative { minutes_before } => *minutes_before,
        ReminderKind::Absolute { at } => return Some(*at),
        ReminderKind::AppStart | ReminderKind::Email { .. } => return None,
    };
    all_day_fire_instant(occ_start, minutes_before, day_start, &chrono::Local)
}

/// Pure, zone-parameterised core of [`all_day_trigger_time`]: read `minutes`
/// as whole DAYS before the occurrence's LOCAL date and anchor at `day_start`
/// that day. Generic over the zone only so tests can pin a fixed offset —
/// production always passes `chrono::Local`. Assumes the start is stored
/// local-midnight-as-UTC (every all-day producer does).
///
/// DST-robust so a reminder is never silently dropped: on an ambiguous
/// fall-back hour (the wall clock repeats) it takes the earlier instant; in a
/// spring-forward gap (the chosen wall time doesn't exist that day) it steps
/// past the ≤1 h gap. Only reachable at all if the user sets `day_start` to a
/// clock time inside their DST transition — the `00:00` default never is.
fn all_day_fire_instant<Tz: TimeZone>(
    occ_start: DateTime<Utc>,
    minutes: i64,
    day_start: NaiveTime,
    tz: &Tz,
) -> Option<DateTime<Utc>> {
    let event_day = occ_start.with_timezone(tz).date_naive();
    let days = ((minutes as f64) / 1440.0).round() as i64;
    let target_day = event_day - ChronoDuration::days(days);
    let naive = NaiveDateTime::new(target_day, day_start);
    tz.from_local_datetime(&naive)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(naive + ChronoDuration::hours(1)))
                .earliest()
        })
        .map(|local| local.with_timezone(&Utc))
}

/// The day-carryover time-of-day that anchors all-day / birthday reminders,
/// read from the synced `tasks.dayStartTrigger` pref. `'HH:MM'` parses to that
/// time; the `'00:00'` default and `'app-start'` (no clock time) both anchor at
/// midnight — a midnight-triggered reminder still surfaces at app-start via the
/// catch-up filter.
fn day_start_time(db: &SharedConn) -> NaiveTime {
    let midnight = NaiveTime::from_hms_opt(0, 0, 0).expect("00:00 is valid");
    UserPrefsRepo::new(db)
        .get("tasks.dayStartTrigger")
        .ok()
        .flatten()
        .as_deref()
        .and_then(parse_local_time)
        .unwrap_or(midnight)
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

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::{EventRecurrence, Reminder, ReminderKind, Task, TaskStatus, TaskUser};
    use chrono::{Datelike, NaiveDate, NaiveTime, Timelike};

    fn prefs_db() -> SharedConn {
        crate::db::DbHandle::open_in_memory()
            .expect("in-memory db")
            .shared()
    }

    fn new_event_without_reminders() -> NewEvent {
        NewEvent {
            title: "Dentist".into(),
            description: None,
            location: None,
            start: Utc.with_ymd_and_hms(2026, 9, 4, 9, 0, 0).unwrap(),
            end: Utc.with_ymd_and_hms(2026, 9, 4, 10, 0, 0).unwrap(),
            all_day: false,
            recurrence: None,
            color_label: None,
            color_hex: None,
            reminders: Vec::new(),
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
        }
    }

    fn one_hour_before() -> Vec<Reminder> {
        vec![Reminder {
            kind: ReminderKind::Relative { minutes_before: 60 },
            sound: None,
        }]
    }

    fn store_defaults(db: &SharedConn, calendar_id: &str, defaults: &[DefaultReminder]) {
        UserPrefsRepo::new(db)
            .set(
                &format!("calendar.{calendar_id}.defaultReminders"),
                &serde_json::to_string(defaults).unwrap(),
            )
            .unwrap();
    }

    /// A default entry that stays in Aperio (the overlay).
    fn local(reminder: Reminder) -> DefaultReminder {
        DefaultReminder {
            reminder,
            attach: false,
        }
    }

    /// A default entry written into new appointments.
    fn attached(reminder: Reminder) -> DefaultReminder {
        DefaultReminder {
            reminder,
            attach: true,
        }
    }

    fn five_minutes_before() -> Reminder {
        Reminder {
            kind: ReminderKind::Relative { minutes_before: 5 },
            sound: None,
        }
    }

    /// The feature itself: an attach entry + unset reminders → the entry
    /// becomes the appointment's own reminder. The in-Aperio entry beside it
    /// never touches the event.
    #[test]
    fn attach_entries_are_written_into_a_new_event_left_without_reminders() {
        let db = prefs_db();
        store_defaults(
            &db,
            "cal",
            &[
                attached(one_hour_before()[0].clone()),
                local(five_minutes_before()),
            ],
        );
        let mut event = new_event_without_reminders();
        assert!(apply_default_reminder_policy(&db, "cal", &mut event, true));
        assert_eq!(event.reminders, one_hour_before());
    }

    /// Entries that stay in Aperio leave the wire alone — the event is created
    /// reminder-less, exactly as before the choice existed.
    #[test]
    fn in_aperio_entries_leave_a_new_event_reminderless() {
        let db = prefs_db();
        store_defaults(&db, "cal", &[local(one_hour_before()[0].clone())]);
        let mut event = new_event_without_reminders();
        assert!(!apply_default_reminder_policy(&db, "cal", &mut event, true));
        assert!(event.reminders.is_empty());
    }

    /// A choice the user made is never overridden — neither "no reminder at
    /// all" (unset = false with an empty list) nor a reminder of their own.
    #[test]
    fn an_explicit_reminder_choice_is_never_overridden() {
        let db = prefs_db();
        store_defaults(&db, "cal", &[attached(one_hour_before()[0].clone())]);

        let mut none_on_purpose = new_event_without_reminders();
        assert!(!apply_default_reminder_policy(
            &db,
            "cal",
            &mut none_on_purpose,
            false
        ));
        assert!(none_on_purpose.reminders.is_empty());

        let own = vec![five_minutes_before()];
        let mut with_own = new_event_without_reminders();
        with_own.reminders = own.clone();
        assert!(!apply_default_reminder_policy(
            &db,
            "cal",
            &mut with_own,
            true
        ));
        assert_eq!(with_own.reminders, own);
    }

    /// Nothing to attach — no defaults, or a list cleared on purpose — changes
    /// nothing.
    #[test]
    fn no_attach_entries_changes_nothing() {
        let db = prefs_db();
        let mut event = new_event_without_reminders();
        assert!(!apply_default_reminder_policy(&db, "cal", &mut event, true));
        assert!(event.reminders.is_empty());
        store_defaults(&db, "cal", &[]);
        assert!(!apply_default_reminder_policy(&db, "cal", &mut event, true));
        assert!(event.reminders.is_empty());
    }

    /// The merge rule in one place: own reminders first; attach entries only
    /// when there are none of its own; in-Aperio entries always, once.
    #[test]
    fn effective_reminders_merge_own_attach_and_in_aperio_entries() {
        let hour = one_hour_before()[0].clone();
        let five = five_minutes_before();
        let defaults = [attached(hour.clone()), local(five.clone())];
        // No own reminders: the attach entry stands in, the in-Aperio one rides on top.
        assert_eq!(
            effective_reminders(&[], &defaults),
            vec![hour.clone(), five.clone()]
        );
        // Own reminders: the attach entry is skipped, the in-Aperio one still fires.
        let own = vec![Reminder {
            kind: ReminderKind::Relative { minutes_before: 10 },
            sound: None,
        }];
        assert_eq!(
            effective_reminders(&own, &defaults),
            vec![own[0].clone(), five.clone()]
        );
        // The same reminder is never doubled.
        assert_eq!(
            effective_reminders(std::slice::from_ref(&five), &defaults),
            vec![five]
        );
    }

    /// The stored shape is the one the settings UI writes, and a list written
    /// before the placement existed still reads — as an in-Aperio entry, which
    /// is the behaviour it always had. An older device parsing a NEWER list
    /// does the same: serde ignores the unknown field, so the entry overlays
    /// there instead of being lost.
    #[test]
    fn the_stored_list_round_trips_with_and_without_the_placement_flag() {
        let stored = r#"[
            {"kind":{"type":"relative","minutes_before":1440},"sound":null},
            {"kind":{"type":"relative","minutes_before":60},"sound":null,"attach":true}
        ]"#;
        let parsed: Vec<DefaultReminder> =
            serde_json::from_str(stored).expect("stored list parses");
        assert_eq!(parsed.len(), 2);
        assert!(!parsed[0].attach, "no flag means the entry stays in Aperio");
        assert!(parsed[1].attach);
        assert_eq!(
            parsed[1].reminder.kind,
            ReminderKind::Relative { minutes_before: 60 }
        );
        assert_eq!(attached_defaults(&parsed), vec![parsed[1].reminder.clone()]);
        // …and what we write is what the UI reads back.
        let written = serde_json::to_string(&parsed).expect("serialises");
        assert_eq!(
            serde_json::from_str::<Vec<DefaultReminder>>(&written).unwrap(),
            parsed
        );
        assert!(written.contains(r#""attach":true"#));
    }

    fn insert_local_event(
        db: &SharedConn,
        id: &str,
        calendar_id: &str,
        reminders_json: &str,
        start: DateTime<Utc>,
    ) {
        let conn = db.lock().unwrap();
        // The event's calendar has to exist (foreign key); one row per id.
        conn.execute(
            "INSERT OR IGNORE INTO calendars (
                id, source, name, color_hex, color_source, read_only,
                default_sound, created_at, updated_at
             ) VALUES (?, 'local', ?, NULL, NULL, 0, NULL, ?, ?)",
            params![
                calendar_id,
                calendar_id,
                start.to_rfc3339(),
                start.to_rfc3339()
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO events (
                id, calendar_id, title, start_utc, end_utc, all_day,
                reminders, sound, attendees, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 0, ?, NULL, '[]', ?, ?)",
            params![
                id,
                calendar_id,
                id,
                start.to_rfc3339(),
                (start + ChronoDuration::hours(1)).to_rfc3339(),
                reminders_json,
                start.to_rfc3339(),
                start.to_rfc3339(),
            ],
        )
        .unwrap();
    }

    fn local_triggers_for(db: &SharedConn, item_id: &str) -> Vec<DateTime<Utc>> {
        let now = Utc::now();
        let mut at: Vec<DateTime<Utc>> =
            enumerate_local_triggers(db, now, now + ChronoDuration::days(2))
                .into_iter()
                .filter(|t| t.item_id == item_id)
                .map(|t| t.trigger_at)
                .collect();
        at.sort();
        at
    }

    /// A local calendar's defaults fire for its reminder-less events. The
    /// overlay used to walk adapter-backed calendars only, so "only in Aperio"
    /// was silent for exactly the calendars that live here.
    #[test]
    fn local_calendar_defaults_overlay_its_reminderless_events() {
        let db = prefs_db();
        let start = Utc::now() + ChronoDuration::hours(6);
        insert_local_event(&db, "ev-empty-list", "cal", "[]", start);
        insert_local_event(&db, "ev-empty-string", "cal", "", start);
        store_defaults(&db, "cal", &[local(one_hour_before()[0].clone())]);
        assert_eq!(
            local_triggers_for(&db, "ev-empty-list"),
            vec![start - ChronoDuration::hours(1)]
        );
        assert_eq!(
            local_triggers_for(&db, "ev-empty-string"),
            vec![start - ChronoDuration::hours(1)]
        );
    }

    /// An in-Aperio entry fires on top of a local event's own reminders; an
    /// attach entry does not — the event has its own.
    #[test]
    fn a_local_events_own_reminders_get_the_in_aperio_entries_on_top() {
        let db = prefs_db();
        let start = Utc::now() + ChronoDuration::hours(6);
        let own = vec![five_minutes_before()];
        insert_local_event(
            &db,
            "ev-own",
            "cal",
            &serde_json::to_string(&own).unwrap(),
            start,
        );
        store_defaults(
            &db,
            "cal",
            &[
                local(one_hour_before()[0].clone()),
                attached(Reminder {
                    kind: ReminderKind::Relative { minutes_before: 30 },
                    sound: None,
                }),
            ],
        );
        assert_eq!(
            local_triggers_for(&db, "ev-own"),
            vec![
                start - ChronoDuration::hours(1),
                start - ChronoDuration::minutes(5)
            ]
        );
    }

    /// No defaults — never configured, or cleared on purpose — means a
    /// reminder-less local event stays silent, as it always did.
    #[test]
    fn a_local_calendar_without_defaults_stays_silent() {
        let db = prefs_db();
        let start = Utc::now() + ChronoDuration::hours(6);
        insert_local_event(&db, "ev-quiet", "cal", "[]", start);
        assert!(local_triggers_for(&db, "ev-quiet").is_empty());
        store_defaults(&db, "cal", &[]);
        assert!(local_triggers_for(&db, "ev-quiet").is_empty());
    }

    /// A zoned weekly series keeps its WALL CLOCK across a DST boundary.
    ///
    /// The bug this pins down: the expansion pinned DTSTART to UTC and threw
    /// `tzid` away, so a series authored at 09:00 Berlin in winter fired its
    /// summer occurrences at 08:00 local — every reminder an hour early, and
    /// near midnight a day out. `EventRecurrence::tzid` documents exactly that,
    /// and nothing honoured it.
    #[test]
    fn a_zoned_series_keeps_its_wall_clock_across_dst() {
        // 09:00 Europe/Berlin on a winter Monday is 08:00 UTC (UTC+1).
        let winter = Utc.with_ymd_and_hms(2026, 1, 5, 8, 0, 0).unwrap();
        let occurrences = expand_occurrences(
            winter,
            "FREQ=WEEKLY;COUNT=30",
            &[],
            Some("Europe/Berlin"),
            winter,
            Utc.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap(),
        );
        // A summer Monday: 09:00 Berlin is 07:00 UTC (UTC+2). Pinned to UTC the
        // expansion would put it at 08:00 UTC, i.e. 10:00 local.
        let july = occurrences
            .iter()
            .find(|o| o.month() == 7)
            .expect("the series reaches July");
        assert_eq!(july.hour(), 7, "summer occurrence should be 07:00 UTC");
        // …and the winter ones are untouched.
        assert_eq!(occurrences[0].hour(), 8);
    }

    /// A series with no zone still expands, in UTC, exactly as before.
    #[test]
    fn a_floating_series_expands_in_utc() {
        let start = Utc.with_ymd_and_hms(2026, 1, 5, 8, 0, 0).unwrap();
        let occurrences = expand_occurrences(
            start,
            "FREQ=WEEKLY;COUNT=30",
            &[],
            None,
            start,
            Utc.with_ymd_and_hms(2026, 12, 31, 0, 0, 0).unwrap(),
        );
        let july = occurrences
            .iter()
            .find(|o| o.month() == 7)
            .expect("the series reaches July");
        assert_eq!(july.hour(), 8);
    }

    /// An unreadable zone degrades to UTC rather than dropping the series —
    /// the behaviour everything had before the zone was honoured at all.
    #[test]
    fn an_unknown_zone_falls_back_to_utc() {
        let start = Utc.with_ymd_and_hms(2026, 1, 5, 8, 0, 0).unwrap();
        let occurrences = expand_occurrences(
            start,
            "FREQ=WEEKLY;COUNT=4",
            &[],
            Some("Mars/Olympus_Mons"),
            start,
            Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap(),
        );
        assert_eq!(occurrences.len(), 4);
        assert!(occurrences.iter().all(|o| o.hour() == 8));
    }

    fn user(id: &str) -> TaskUser {
        TaskUser {
            id: id.to_string(),
            name: id.to_string(),
            email: None,
        }
    }

    #[test]
    fn ownership_filter_matches_day_start_rule() {
        let me = user("me");
        let other = user("other");
        // No account identity → everything is "mine" (local / personal lists).
        assert!(is_mine_or_unassigned(std::slice::from_ref(&other), None));
        // Unassigned → mine.
        assert!(is_mine_or_unassigned(&[], Some(&me)));
        // Assigned to me (possibly alongside others) → mine.
        assert!(is_mine_or_unassigned(std::slice::from_ref(&me), Some(&me)));
        assert!(is_mine_or_unassigned(
            &[me.clone(), other.clone()],
            Some(&me)
        ));
        // Assigned only to others → NOT mine (no reminder fires).
        assert!(!is_mine_or_unassigned(&[other], Some(&me)));
    }

    /// Test wrappers that inject an empty `SoundPrefs` so the existing
    /// trigger-shape assertions stay terse. These tests assert on timing
    /// and counts, not the resolved sound (every occurrence resolves to
    /// System with an empty snapshot — that precedence is covered in
    /// `crate::sound`'s own unit tests).
    fn ev_triggers(
        events: &[Event],
        calendar_defaults: &[DefaultReminder],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Vec<Trigger> {
        event_triggers(
            events,
            calendar_defaults,
            &SoundPrefs::default(),
            window_start,
            window_end,
            // Day-carryover anchor; only consulted for all-day events, which
            // the all-day test constructs explicitly.
            NaiveTime::from_hms_opt(9, 0, 0).expect("9:00 is valid"),
        )
    }

    fn tk_triggers(
        tasks: &[Task],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Vec<Trigger> {
        task_triggers(tasks, &SoundPrefs::default(), window_start, window_end)
    }

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
            color_hex: None,
            reminders,
            sound: None,
            attendees: Vec::new(),
            send_invitations: false,
            truncate_tail_overrides: false,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            etag: None,
            organizer: None,
            attendee_responses: Vec::new(),
            cancelled: false,
        }
    }

    /// An all-day event on `day`, stored the way the app stores all-day events:
    /// LOCAL midnight → UTC. Keeps the day unambiguous regardless of the test
    /// machine's timezone.
    fn make_all_day_event(day: NaiveDate, reminders: Vec<Reminder>) -> Event {
        let start_local = chrono::Local
            .from_local_datetime(&day.and_hms_opt(0, 0, 0).unwrap())
            .single()
            .unwrap();
        let mut ev = make_event(reminders);
        ev.all_day = true;
        ev.start = start_local.with_timezone(&Utc);
        ev.end = ev.start + ChronoDuration::days(1);
        ev
    }

    #[test]
    fn cancelled_event_schedules_no_reminders() {
        // A cancelled meeting never nags — even with an explicit reminder, and
        // regardless of the visibility setting (which only governs rendering).
        let mut ev = make_event(vec![rel(30)]);
        ev.cancelled = true;
        let ws = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let we = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert!(ev_triggers(&[ev], &[], ws, we).is_empty());
    }

    #[test]
    fn cancelled_event_ignores_calendar_default_reminders_too() {
        // The calendar-default fallback must not resurrect a cancelled event's
        // reminders either.
        let mut ev = make_event(vec![]);
        ev.cancelled = true;
        let ws = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let we = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        assert!(ev_triggers(&[ev], &[local(rel(15))], ws, we).is_empty());
    }

    #[test]
    fn all_day_reminder_fires_at_day_start_not_the_night_before() {
        // "1 hour before" on an all-day event: timed this would ring 23:00 the
        // night before (the reported bug); all-day it must ring at the
        // day-carryover time (9:00 here, per ev_triggers) on the event's OWN day.
        let day = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        let ev = make_all_day_event(day, vec![rel(60)]);
        let ws = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let we = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let triggers = ev_triggers(&[ev], &[], ws, we);
        assert_eq!(triggers.len(), 1);
        let local = chrono::Local.from_utc_datetime(&triggers[0].trigger_at.naive_utc());
        assert_eq!(local.date_naive(), day);
        assert_eq!(local.time(), NaiveTime::from_hms_opt(9, 0, 0).unwrap());
    }

    #[test]
    fn all_day_fire_instant_is_correct_west_of_utc() {
        // Deterministic west-of-UTC guard (independent of the test machine's
        // timezone; CI runs UTC, where the "one day early" bug is invisible). An
        // all-day start stored local-midnight-as-UTC in UTC−5 is 05:00Z.
        let west = chrono::FixedOffset::west_opt(5 * 3600).unwrap();
        let occ_start = Utc.with_ymd_and_hms(2026, 5, 20, 5, 0, 0).unwrap();
        let day_start = NaiveTime::from_hms_opt(9, 0, 0).unwrap();

        // "1 week before" → 7 days earlier at day-start, in that same zone.
        let at = all_day_fire_instant(occ_start, 7 * 24 * 60, day_start, &west).unwrap();
        let local = at.with_timezone(&west);
        assert_eq!(
            local.date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 13).unwrap()
        );
        assert_eq!(local.time(), day_start);

        // "1 h before" collapses to 0 days → the event's OWN day at day-start,
        // NOT the day before (the exact west-of-UTC failure mode).
        let same = all_day_fire_instant(occ_start, 60, day_start, &west).unwrap();
        assert_eq!(
            same.with_timezone(&west).date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()
        );
    }

    #[test]
    fn all_day_reminder_reads_the_offset_as_whole_days() {
        // "1 week before" (10080 min) → 7 days before at the day-start time.
        let day = NaiveDate::from_ymd_opt(2026, 5, 20).unwrap();
        let ev = make_all_day_event(day, vec![rel(7 * 24 * 60)]);
        let ws = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
        let we = Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let triggers = ev_triggers(&[ev], &[], ws, we);
        assert_eq!(triggers.len(), 1);
        let local = chrono::Local.from_utc_datetime(&triggers[0].trigger_at.naive_utc());
        assert_eq!(
            local.date_naive(),
            NaiveDate::from_ymd_opt(2026, 5, 13).unwrap()
        );
        assert_eq!(local.time(), NaiveTime::from_hms_opt(9, 0, 0).unwrap());
    }

    #[tokio::test]
    async fn birthday_calendar_reminders_default_on_and_fire_at_day_start() {
        use crate::cache::CacheStore;
        use crate::db::DbHandle;
        use crate::user_prefs::UserPrefsRepo;
        use adapter_local::LocalAdapter;
        use cal_core::{ContactsFeature, NewContact};
        use chrono::Datelike;

        let db = DbHandle::open_in_memory().expect("in-memory db");
        let shared = db.shared();
        let adapter = LocalAdapter::new(db.shared());
        let cache = Arc::new(CacheStore::new(db.clone()));
        let registry = Arc::new(AdapterRegistry::new(
            Arc::new(plugin_core::PluginManager::new("0.1.0")),
            Arc::new(sync_engine::test_support::FakeSecrets::default()),
        ));

        // A contact whose birthday is ~30 days out, so a "1 week before" reminder
        // lands comfortably inside the 90-day scan horizon. Birth year 2000 (a
        // leap year) keeps `from_ymd_opt` panic-free even on a Feb-29 anniversary.
        let list = adapter
            .create_contact_list("Friends", None, None)
            .expect("create list");
        let target = (Utc::now() + ChronoDuration::days(30))
            .with_timezone(&chrono::Local)
            .date_naive();
        let bday = NaiveDate::from_ymd_opt(2000, target.month(), target.day()).unwrap();
        adapter
            .create_contact(
                &list.id,
                NewContact {
                    urls: Vec::new(),
                    anniversary: None,
                    job_title: None,
                    department: None,
                    name_prefix: None,
                    name_suffix: None,
                    display_name: "Alex".into(),
                    given_name: None,
                    family_name: None,
                    organization: None,
                    emails: vec![],
                    phone_numbers: vec![],
                    birthday: Some(bday),
                    notes: None,
                    addresses: vec![],
                    members: None,
                    photo: None,
                },
            )
            .await
            .expect("create contact");

        let cal_id = crate::birthdays::birthday_calendar_id(&list.id);

        // Nothing configured → the BUILT-IN default fires, on the day itself
        // at the day-change time. This used to assert silence, and silence is
        // what made the feature look broken to the people who wanted it: a
        // birthday calendar exists because somebody wants to be told.
        let built_in = enumerate_birthday_triggers(&adapter, &registry, &cache, &shared).await;
        assert_eq!(
            built_in.len(),
            1,
            "an unconfigured birthday calendar reminds on the day itself"
        );
        assert_eq!(
            built_in[0]
                .trigger_at
                .with_timezone(&chrono::Local)
                .date_naive(),
            target,
            "the built-in default fires ON the birthday, not before it"
        );

        // Cleared ON PURPOSE stays silent — otherwise the built-in default
        // would be impossible to switch off, which is a worse bug than the
        // one it fixes.
        UserPrefsRepo::new(&shared)
            .set(&format!("calendar.{cal_id}.defaultReminders"), "[]")
            .expect("set pref");
        let silenced = enumerate_birthday_triggers(&adapter, &registry, &cache, &shared).await;
        assert!(
            silenced.is_empty(),
            "an emptied list must stay empty, not fall back to the built-in"
        );

        // Configure "one week before" (10080 min) for this birthday calendar.
        UserPrefsRepo::new(&shared)
            .set(
                &format!("calendar.{cal_id}.defaultReminders"),
                r#"[{"kind":{"type":"relative","minutes_before":10080}}]"#,
            )
            .expect("set pref");

        let triggers = enumerate_birthday_triggers(&adapter, &registry, &cache, &shared).await;
        assert!(
            !triggers.is_empty(),
            "a configured default reminder now fires"
        );
        for t in &triggers {
            let local = chrono::Local.from_utc_datetime(&t.trigger_at.naive_utc());
            // Fires AT the day-carryover time (default 00:00), not a clock offset.
            assert_eq!(local.time(), NaiveTime::from_hms_opt(0, 0, 0).unwrap());
            // …a whole 7 days before the birthday's month/day.
            let anniversary = local.date_naive() + ChronoDuration::days(7);
            assert_eq!(anniversary.month(), bday.month());
            assert_eq!(anniversary.day(), bday.day());
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
            effort: cal_core::TaskEffort::Medium,
            deadline_reminder_days: None,
            scheduled_date,
            scheduled_time,
            scheduled_end_time: None,
            deadline_date,
            deadline_time,
            recurrence: None,
            resurface_date: None,
            series_id: None,
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
    fn event_with_explicit_reminder_uses_it_and_ignores_an_attach_default() {
        // The event carries its own VALARM-equivalent. An attach entry would
        // have been written into the event had it been created here, so it
        // stands in only for events that have none of their own.
        let ev = make_event(vec![rel(15)]);
        let (ws, we) = wide_window();
        let triggers = ev_triggers(&[ev], &[attached(rel(60))], ws, we);
        assert_eq!(triggers.len(), 1);
        // 8:00 start − 15 min = 7:45.
        assert_eq!(
            triggers[0].trigger_at,
            Utc.with_ymd_and_hms(2026, 5, 20, 7, 45, 0).unwrap(),
        );
    }

    #[test]
    fn event_with_explicit_reminder_still_gets_an_in_aperio_default_on_top() {
        // An entry that stays in Aperio is the calendar's baseline: it fires
        // for every event, beside whatever the event brought itself.
        let ev = make_event(vec![rel(15)]);
        let (ws, we) = wide_window();
        let mut at: Vec<DateTime<Utc>> = ev_triggers(&[ev], &[local(rel(60))], ws, we)
            .into_iter()
            .map(|t| t.trigger_at)
            .collect();
        at.sort();
        assert_eq!(
            at,
            vec![
                Utc.with_ymd_and_hms(2026, 5, 20, 7, 0, 0).unwrap(),
                Utc.with_ymd_and_hms(2026, 5, 20, 7, 45, 0).unwrap(),
            ],
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
        let triggers = ev_triggers(&[ev], &[local(rel(15))], ws, we);
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
        let triggers = ev_triggers(&[ev], &[], ws, we);
        assert!(triggers.is_empty());
    }

    #[test]
    fn event_with_multiple_defaults_emits_one_trigger_each() {
        let ev = make_event(Vec::new());
        let (ws, we) = wide_window();
        let triggers = ev_triggers(&[ev], &[local(rel(60)), attached(rel(10))], ws, we);
        assert_eq!(triggers.len(), 2);
    }

    fn make_recurring_event(rrule: &str, exceptions: Vec<DateTime<Utc>>) -> Event {
        let mut ev = make_event(vec![rel(15)]);
        ev.recurrence = Some(EventRecurrence {
            rrule: rrule.to_string(),
            exceptions,
            tzid: None,
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
        let triggers = ev_triggers(
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
        let triggers = ev_triggers(
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
        let triggers = ev_triggers(
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
        let triggers = ev_triggers(
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
        let triggers = tk_triggers(&[task], ws, we);
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
        let triggers = tk_triggers(&[task], ws, we);
        assert_eq!(triggers.len(), 1);
        let dt_local = chrono::Local.from_utc_datetime(&triggers[0].trigger_at.naive_utc());
        assert_eq!(dt_local.time(), NaiveTime::from_hms_opt(9, 0, 0).unwrap());
    }

    #[test]
    fn task_without_any_date_emits_nothing() {
        let task = make_task(None, None, None, None, vec![rel(15)]);
        let (ws, we) = task_window();
        assert!(tk_triggers(&[task], ws, we).is_empty());
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
        assert!(tk_triggers(&[task], ws, we).is_empty());
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        });
        let triggers = tk_triggers(
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        });
        let triggers = tk_triggers(
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
            container_id: "cal".into(),
            title: "T".into(),
            body: String::new(),
            trigger_at: now + ChronoDuration::minutes(at_offset_min),
            relevant_until: now + ChronoDuration::minutes(relevant_offset_min),
            start: now + ChronoDuration::minutes(at_offset_min),
            all_day: false,
            sound: SoundConfig::default(),
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
        let triggers = ev_triggers(&[ev], &[], ws, we);
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
        let triggers = tk_triggers(&[task], ws, we);
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
            anchor: Default::default(),
            placement: Default::default(),
            fixed_dates: None,
        });
        let triggers = tk_triggers(
            &[task],
            Utc.with_ymd_and_hms(2026, 5, 19, 0, 0, 0).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap(),
        );
        // 2026-05-20, 05-27, 06-03 — 3 occurrences, 06-10 excluded.
        assert_eq!(triggers.len(), 3, "UNTIL=06-03 should leave three triggers");
    }
}
