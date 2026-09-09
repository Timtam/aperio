//! Reminder scheduler (DESIGN.md §14) — the Tauri delivery shell.
//!
//! The trigger ENUMERATION (which reminders fire, and when) now lives in
//! [`host_core::reminders`], shared with the mobile cal-ffi reminder surface.
//! This module keeps the desktop-only delivery:
//!   1. a single background tokio task that sleeps until the next trigger;
//!   2. dispatch via `tauri-plugin-notification` (+ custom-sound playback);
//!   3. an in-process "already fired" set so a restart within the same minute
//!      doesn't double-notify.
//!
//! Re-computation is signalled through a [`tokio::sync::Notify`]: the CRUD
//! command layer calls [`ReminderScheduler::invalidate`] after any event/task
//! mutation; the worker wakes, throws away its pending wait, and re-scans.
//! External-adapter triggers are kept in a TTL cache so a flurry of local
//! mutations doesn't slam every registered server with full fan-out fetches.
//!
//! Storage of "already fired" reminders is in-memory only; a crash between a
//! fire and the next scan can re-deliver a reminder.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adapter_local::{LocalAdapter, SharedConn};
use cal_core::SoundSource;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tauri::{AppHandle, Runtime};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::audio::AudioPlayer;
use crate::cache::CacheStore;
use crate::registry::AdapterRegistry;
// Trigger enumeration + the shared types it produces. Re-exported so existing
// `crate::reminders::{Trigger, ItemKind, UpcomingReminder}` references (and the
// `get_upcoming_reminders` command) keep resolving.
use host_core::reminders::{
    catchup_eligible, enumerate_app_start_triggers, enumerate_birthday_triggers,
    enumerate_external_triggers, enumerate_local_triggers, CATCH_UP_GRACE,
};
pub use host_core::reminders::{ItemKind, Trigger, UpcomingReminder};

/// Identifier for a single fired reminder. Two reminders fire at the "same"
/// trigger only if they share item id AND timestamp.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct FiredKey {
    item_id: String,
    trigger_iso: String,
}

/// Cache slot for the external-adapter scan. The TTL keeps the scheduler from
/// refetching iCloud / Graph / EWS on every local `invalidate()` call — most
/// mutations don't change external state, and the small `EXTERNAL_TRIGGERS_TTL`
/// window means a real remote change shows up within a few minutes either way.
struct ExternalTriggerCache {
    fetched_at: Instant,
    triggers: Vec<Trigger>,
}

/// The words a notification body needs, pushed down from the frontend.
///
/// The host has no i18n — the same reason `set_tray_labels` exists — and
/// reminders are the one place where that showed as a WRONG statement rather
/// than an English one: `format_event_body` renders every body as `%H:%M`, and
/// an all-day event starts at local midnight, so the desktop announced
/// "00:00". Mobile has said "Ganztägig" since it shipped, and its
/// `notificationBody` names this bug in its own doc comment.
///
/// `Trigger` has carried what is needed all along — `all_day`, `start` and
/// `relevant_until`, whose own doc comment says they exist so a notification
/// can say «Ganztägig · 24. Juni bis 26. Juni» instead of a meaningless
/// "00:00". Only the words were missing.
///
/// Month names come from the frontend's `Intl`, and `day_month` carries the
/// ORDER, because that is not the same in every language: "24. Juni" against
/// "June 24". Deriving it here would mean a date-formatting library and a
/// second answer to "which language is this" — the frontend already knows both.
#[derive(Debug, Clone)]
pub struct ReminderLabels {
    /// `dialogs.reminders.allDay`.
    pub all_day: String,
    /// `dialogs.reminders.allDayRange`, with `{{from}}` and `{{to}}`.
    pub all_day_range: String,
    /// `dialogs.reminders.dayMonth`, with `{{day}}` and `{{month}}`.
    pub day_month: String,
    /// January … December, in the app's language. Twelve entries; a shorter
    /// list makes the range fall back to the plain all-day line rather than
    /// index out of bounds.
    pub months: Vec<String>,
}

pub struct ReminderScheduler {
    db: SharedConn,
    registry: Arc<AdapterRegistry>,
    /// Snapshot cache for external contact books — read (never network-fetched)
    /// when synthesising birthday-calendar reminders. See
    /// [`enumerate_birthday_triggers`].
    cache: Arc<CacheStore>,
    invalidate: Arc<Notify>,
    fired: Arc<Mutex<HashSet<FiredKey>>>,
    external_cache: Arc<Mutex<Option<ExternalTriggerCache>>>,
    /// Calendar ids the user UNCHECKED in the sidebar (device-local
    /// localStorage, pushed from the frontend via `set_hidden_calendars`). Event
    /// reminders on these calendars are suppressed — hiding a calendar silences
    /// its reminders too. Empty until the frontend pushes; tasks are unaffected
    /// (a task list isn't a calendar).
    hidden_calendars: Arc<Mutex<HashSet<String>>>,
    /// The notification wording, pushed from the frontend via
    /// `set_reminder_labels` once i18n is up and on every language change —
    /// exactly as `hidden_calendars` above is pushed. `None` until then, and a
    /// body that would need words is then OMITTED rather than guessed: a
    /// notification with only a title says less than it could, but it does not
    /// say something false.
    labels: Arc<Mutex<Option<ReminderLabels>>>,
    /// `<data_dir>/assets/sounds/` — where custom sound files live.
    /// Used by `fire` to resolve a `SoundSource::Custom` hash to a path.
    sounds_dir: PathBuf,
    /// Handle to the process-wide audio thread for custom-sound playback.
    audio: AudioPlayer,
}

/// What a notification should say under the title.
///
/// Free rather than a method so it can be tested without a database, a
/// registry and an audio thread — the part that needs the world is one mutex
/// read, and it stays in the caller.
///
/// `None` means "no body", and that is the honest answer for an all-day event
/// before the frontend has pushed any words. The alternative is what this
/// replaces: `t.body`, which for an all-day event is `"00:00"` — a time nobody
/// set and the event does not have.
fn notification_body(t: &Trigger, labels: Option<&ReminderLabels>) -> Option<String> {
    if !t.all_day {
        // A time, formatted from an instant. No words in it, so no language
        // question — this is the one body the host can build alone.
        return Some(t.body.clone());
    }
    let labels = labels?;
    // Mirrors `allDayReminderDays` in `@aperio/shared`: the end is EXCLUSIVE
    // (the next midnight), so a one-day event spans exactly one.
    let days = (t.relevant_until - t.start).num_days().max(1);
    if days <= 1 || labels.months.len() != 12 {
        // A short month list is a frontend that pushed something unexpected.
        // Falling back to the plain line says less than it could and nothing
        // that is wrong, which is the trade this whole function makes.
        return Some(labels.all_day.clone());
    }
    // Step back one day: the end is exclusive, the user reads the last day it
    // actually covers.
    let last = t.relevant_until - ChronoDuration::days(1);
    Some(
        labels
            .all_day_range
            .replace("{{from}}", &day_month(labels, t.start))
            .replace("{{to}}", &day_month(labels, last)),
    )
}

/// One date as the app's language writes it: "24. Juni", "June 24".
///
/// The ORDER comes from the template, not from here — that is the whole reason
/// `day_month` is pushed down rather than assembled locally.
///
/// The instant is read in UTC on purpose. An all-day occurrence's `start` is
/// local midnight already stored as UTC, so reading it back in UTC gives the
/// day the user means; reading it in the machine's zone would be the core's
/// device-zone question all over again (DESIGN §4.5), one layer up.
fn day_month(labels: &ReminderLabels, at: DateTime<Utc>) -> String {
    use chrono::Datelike;
    let month = labels
        .months
        .get(at.month0() as usize)
        .cloned()
        .unwrap_or_default();
    labels
        .day_month
        .replace("{{day}}", &at.day().to_string())
        .replace("{{month}}", &month)
}

impl ReminderScheduler {
    /// Start the worker loop on Tauri's async runtime.
    ///
    /// We can't use `tokio::spawn` here because `tauri::Builder::setup` fires on
    /// the main thread without an active tokio runtime context.
    /// `tauri::async_runtime::spawn` resolves against the runtime Tauri installs
    /// itself (tokio by default) — the same one that powers `#[tauri::command]`
    /// async handlers — so the `tokio::select!` + `tokio::time::sleep` calls
    /// inside the worker still work as expected.
    pub fn spawn<R: Runtime>(
        db: SharedConn,
        registry: Arc<AdapterRegistry>,
        cache: Arc<CacheStore>,
        sounds_dir: PathBuf,
        audio: AudioPlayer,
        app: AppHandle<R>,
    ) -> Arc<Self> {
        let scheduler = Arc::new(Self {
            db,
            registry,
            cache,
            invalidate: Arc::new(Notify::new()),
            fired: Arc::new(Mutex::new(HashSet::new())),
            external_cache: Arc::new(Mutex::new(None)),
            hidden_calendars: Arc::new(Mutex::new(HashSet::new())),
            labels: Arc::new(Mutex::new(None)),
            sounds_dir,
            audio,
        });
        let worker = scheduler.clone();
        tauri::async_runtime::spawn(async move {
            // First sweep covers the "fired while we were offline" case for
            // app_start reminders. Local-only — external adapters don't carry
            // the AppStart kind on the wire.
            worker.fire_app_start_reminders(&app);
            worker.worker_loop(app).await;
        });
        scheduler
    }

    /// Wake the worker so it re-scans. Safe to call from any thread; multiple
    /// back-to-back calls are coalesced by `Notify`. Local triggers re-scan on
    /// every wake; external triggers come from the TTL cache unless the caller
    /// also calls `invalidate_external_cache`.
    pub fn invalidate(&self) {
        self.invalidate.notify_one();
    }

    /// Drop the external-trigger snapshot so the next scan re-fans out to every
    /// adapter. Used by settings flows whose change affects how external events
    /// resolve to Triggers — most notably "Standard-Hinweise" for a calendar
    /// (per-calendar default reminders), where the cached Triggers were
    /// materialised against the OLD defaults.
    ///
    /// Pairs with `invalidate()`: the caller usually wants both — clear the
    /// cache, then wake the worker so it picks up the change immediately.
    pub fn invalidate_external_cache(&self) {
        let mut guard = self.external_cache.lock().expect("external cache poison");
        *guard = None;
    }

    /// Replace the set of hidden (sidebar-unchecked) calendars and wake the
    /// worker so the change takes effect at once. Pushed by the frontend
    /// whenever the sidebar calendar selection changes (+ on startup). Event
    /// reminders on a hidden calendar are then dropped; task reminders are
    /// unaffected.
    pub fn set_hidden_calendars(&self, ids: Vec<String>) {
        {
            let mut guard = self
                .hidden_calendars
                .lock()
                .expect("hidden calendars poison");
            *guard = ids.into_iter().collect();
        }
        self.invalidate.notify_one();
    }

    /// Push the notification wording down from the frontend. Called once i18n
    /// is up and again on every language change.
    ///
    /// Deliberately does NOT invalidate: the words change what a notification
    /// SAYS, never which ones fire or when, and a language switch should not
    /// cost a full re-scan.
    pub fn set_labels(&self, labels: ReminderLabels) {
        *self.labels.lock().expect("reminder labels poison") = Some(labels);
    }

    /// What the notification should say under the title.
    fn notification_body(&self, t: &Trigger) -> Option<String> {
        let guard = self.labels.lock().expect("reminder labels poison");
        notification_body(t, guard.as_ref())
    }

    /// A clone of the hidden-calendar set, for filtering a trigger list without
    /// holding the lock across the scan.
    fn hidden_calendars_snapshot(&self) -> HashSet<String> {
        self.hidden_calendars
            .lock()
            .expect("hidden calendars poison")
            .clone()
    }

    /// True when this trigger is an EVENT reminder on a hidden calendar — the
    /// one case visibility suppresses. Task triggers carry a list_id in
    /// `container_id`; visibility is calendar-only, so they always pass.
    fn suppressed_by_visibility(t: &Trigger, hidden: &HashSet<String>) -> bool {
        t.item_kind == ItemKind::Event && hidden.contains(&t.container_id)
    }

    /// Snapshot reminder triggers for the Ctrl+Shift+R overview dialog.
    /// Includes both already-passed and upcoming triggers within a generous
    /// window so the user can review what fired recently and what's pending.
    /// Sorted ascending by trigger time and capped at `limit`.
    pub async fn upcoming(&self, limit: usize) -> Vec<UpcomingReminder> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::days(OVERVIEW_PAST_DAYS);
        let latest = now + ChronoDuration::days(OVERVIEW_FUTURE_DAYS);
        let mut triggers = self.collect_triggers_in_window(earliest, latest).await;
        let hidden = self.hidden_calendars_snapshot();
        triggers.retain(|t| !Self::suppressed_by_visibility(t, &hidden));
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
                    // Nothing scheduled within the horizon. Block on an
                    // invalidate, but with a periodic fallback so a freshly-
                    // added external reminder that landed AFTER the cache went
                    // stale still gets discovered.
                    tokio::select! {
                        _ = self.invalidate.notified() => continue,
                        _ = tokio::time::sleep(EMPTY_HORIZON_RETRY) => continue,
                    }
                }
            }
        }
    }

    /// Local SQLite + cached external triggers within `[earliest, latest]`.
    /// Local triggers are read fresh on every call; external come from the TTL
    /// cache (a full-horizon snapshot, sliced to the requested sub-window).
    async fn collect_triggers_in_window(
        &self,
        earliest: DateTime<Utc>,
        latest: DateTime<Utc>,
    ) -> Vec<Trigger> {
        let mut acc = enumerate_local_triggers(&self.db, earliest, latest);
        let external = self.external_triggers_cached_or_fetch().await;
        for t in external {
            if t.trigger_at >= earliest && t.trigger_at <= latest {
                acc.push(t);
            }
        }
        acc
    }

    /// Read the external-trigger cache, or refresh it (via the shared
    /// [`enumerate_external_triggers`]) if absent / stale. The cache holds the
    /// *full* fan-out horizon so the caller can slice any sub-window without
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
        let mut fresh = enumerate_external_triggers(&self.registry, &self.db).await;
        // Synthetic birthday calendars aren't adapters, so the external fan-out
        // above skips them. Fold their configured default reminders in here so
        // they share the same TTL cache (and the same
        // `invalidate_external_cache` after a "Standard-Hinweise" change).
        let local = LocalAdapter::new(self.db.clone());
        fresh.extend(
            enumerate_birthday_triggers(&local, &self.registry, &self.cache, &self.db).await,
        );
        let mut guard = self.external_cache.lock().expect("external cache poison");
        *guard = Some(ExternalTriggerCache {
            fetched_at: Instant::now(),
            triggers: fresh.clone(),
        });
        fresh
    }

    /// Build the list of pending (not-yet-fired, in-the-future or catch-up-
    /// eligible) reminder triggers for the scheduler. Lookahead is bounded by
    /// `MAX_HORIZON_DAYS`; the past bound reaches `CATCH_UP_HORIZON` so reminders
    /// that lapsed while the app was closed still make it into the scan (the
    /// relevance filter then drops the ones whose event has already happened).
    async fn collect_pending_triggers(&self) -> Vec<Trigger> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::days(CATCH_UP_HORIZON);
        let latest = now + ChronoDuration::days(MAX_HORIZON_DAYS);
        let mut out = self.collect_triggers_in_window(earliest, latest).await;

        // De-duplicate (same item_id + same trigger time appearing from both
        // local SQLite and an external adapter — keep the first occurrence).
        let mut seen: HashSet<(String, String)> = HashSet::new();
        out.retain(|t| seen.insert((t.item_id.clone(), t.trigger_at.to_rfc3339())));

        // Catch-up filter. Future triggers always stay (the worker schedules
        // them via tokio::sleep). Past triggers stay only when the event itself
        // is still relevant — see `catchup_eligible`.
        out.retain(|t| catchup_eligible(t, now, CATCH_UP_GRACE));

        // Filter out anything we already fired in this process.
        let hidden = self.hidden_calendars_snapshot();
        let fired = self.fired.lock().expect("fired set poisoned");
        out.retain(|t| {
            !fired.contains(&FiredKey {
                item_id: t.item_id.clone(),
                trigger_iso: t.trigger_at.to_rfc3339(),
            })
        });
        // Hidden calendars (sidebar-unchecked) silence their event reminders.
        out.retain(|t| !Self::suppressed_by_visibility(t, &hidden));
        out
    }

    fn fire<R: Runtime>(&self, app: &AppHandle<R>, t: &Trigger) {
        info!(
            item_kind = ?t.item_kind,
            item_id = %t.item_id,
            "firing reminder"
        );
        let builder = app.notification().builder().title(&t.title);
        let builder = match self.notification_body(t) {
            Some(body) => builder.body(body),
            None => builder,
        };
        // §14.4 playback dispatch:
        //   - System → let the OS play its default notification sound.
        //   - Silent → suppress sound, visual only.
        //   - Custom → play the file ourselves (the notification plugin can't),
        //     so silence the toast to avoid a double sound. A missing file falls
        //     back to the System sound rather than silently dropping the cue.
        let builder = match &t.sound.source {
            SoundSource::System => builder,
            SoundSource::Silent => builder.silent(),
            SoundSource::Custom { sha256 } => {
                match crate::sound_assets::local_sound_path(&self.sounds_dir, sha256) {
                    Some(path) => {
                        self.audio.play_file(path);
                        builder.silent()
                    }
                    None => {
                        warn!(
                            hash = %sha256,
                            "custom sound file missing; falling back to system sound",
                        );
                        builder
                    }
                }
            }
        };
        if let Err(err) = builder.show() {
            warn!(?err, "failed to dispatch notification");
        }
        let mut fired = self.fired.lock().expect("fired set poisoned");
        fired.insert(FiredKey {
            item_id: t.item_id.clone(),
            trigger_iso: t.trigger_at.to_rfc3339(),
        });
    }

    /// Look for `app_start` reminders whose due time has already passed and fire
    /// them at startup. The local-only scan lives in [`host_core::reminders`];
    /// this just dispatches each.
    fn fire_app_start_reminders<R: Runtime>(&self, app: &AppHandle<R>) {
        // Empty until the frontend's first push, so a hidden calendar's
        // (local-only) app_start reminder may still fire once on a cold launch
        // before the push lands; the periodic loop suppresses it thereafter.
        let hidden = self.hidden_calendars_snapshot();
        for t in enumerate_app_start_triggers(&self.db) {
            if Self::suppressed_by_visibility(&t, &hidden) {
                continue;
            }
            self.fire(app, &t);
        }
    }
}

/// How many days into the future the scheduler looks ahead in a single pass. A
/// new mutation invalidates the loop, so the horizon only needs to bridge a
/// quiet stretch.
const MAX_HORIZON_DAYS: i64 = 30;
/// How far back the scheduler reaches for catch-up candidates — reminders whose
/// trigger time lapsed while the app was closed. The relevance filter drops
/// anything where the event is already over, so this is a scan-size guard.
const CATCH_UP_HORIZON: i64 = 7;
/// How far back the Ctrl+Shift+R overview looks for already-passed reminders.
const OVERVIEW_PAST_DAYS: i64 = 7;
/// How far forward the overview shows upcoming reminders. Longer than the
/// scheduler horizon so the user can plan ahead.
const OVERVIEW_FUTURE_DAYS: i64 = 90;
/// Lifetime of the external-trigger snapshot. Five minutes matches the CalDAV
/// listing cache — short enough that a freshly created reminder still becomes
/// eligible to fire within the time the user expects.
const EXTERNAL_TRIGGERS_TTL: Duration = Duration::from_secs(5 * 60);
/// When the local + external scan returns nothing, the worker still wakes up
/// periodically so a freshly-added external reminder that landed AFTER the cache
/// went stale still gets picked up. Matches the cache TTL.
const EMPTY_HORIZON_RETRY: Duration = EXTERNAL_TRIGGERS_TTL;

/// Thin alias for the shared scheduler handle that command modules pull out of
/// `tauri::State`.
pub type SchedulerHandle = Arc<ReminderScheduler>;

#[cfg(test)]
mod tests {
    use super::*;
    use cal_core::SoundConfig;
    use host_core::reminders::ItemKind;

    fn labels() -> ReminderLabels {
        ReminderLabels {
            all_day: "Ganztägig".into(),
            all_day_range: "Ganztägig · {{from}} bis {{to}}".into(),
            day_month: "{{day}}. {{month}}".into(),
            months: [
                "Januar",
                "Februar",
                "März",
                "April",
                "Mai",
                "Juni",
                "Juli",
                "August",
                "September",
                "Oktober",
                "November",
                "Dezember",
            ]
            .iter()
            .map(|m| (*m).to_string())
            .collect(),
        }
    }

    fn at(rfc3339: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(rfc3339)
            .expect("a literal RFC-3339 instant")
            .with_timezone(&Utc)
    }

    /// A trigger as the enumeration produces one. `body` is what
    /// `format_event_body` writes — `%H:%M` of the start, which for an all-day
    /// occurrence is midnight.
    fn trigger(all_day: bool, start: &str, until: &str, body: &str) -> Trigger {
        Trigger {
            item_id: "e1".into(),
            item_kind: ItemKind::Event,
            container_id: "cal1".into(),
            title: "Geburtstag von Kim".into(),
            body: body.into(),
            trigger_at: at(start),
            relevant_until: at(until),
            start: at(start),
            all_day,
            sound: SoundConfig {
                source: cal_core::SoundSource::System,
                volume: 100,
            },
        }
    }

    #[test]
    fn a_timed_event_keeps_its_time() {
        // The one body the host can build alone: no words in it, so no
        // language question. It does not wait for the frontend either.
        let t = trigger(
            false,
            "2026-06-24T12:30:00Z",
            "2026-06-24T13:30:00Z",
            "14:30",
        );
        assert_eq!(
            notification_body(&t, None).as_deref(),
            Some("14:30"),
            "a timed reminder must not depend on pushed labels",
        );
    }

    #[test]
    fn an_all_day_event_says_all_day_and_never_a_time() {
        // The bug. `body` is "00:00" because an all-day occurrence starts at
        // local midnight, and the desktop announced exactly that — for a
        // birthday, out loud, through a screen reader.
        let t = trigger(
            true,
            "2026-06-24T00:00:00Z",
            "2026-06-25T00:00:00Z",
            "00:00",
        );
        assert_eq!(
            notification_body(&t, Some(&labels())).as_deref(),
            Some("Ganztägig"),
        );
    }

    #[test]
    fn a_multi_day_all_day_event_says_the_span_the_user_sees() {
        // The end is EXCLUSIVE — the next midnight — so the last day a reader
        // is told about is one step back from it. 24th through 26th inclusive
        // ends at midnight on the 27th.
        let t = trigger(
            true,
            "2026-06-24T00:00:00Z",
            "2026-06-27T00:00:00Z",
            "00:00",
        );
        assert_eq!(
            notification_body(&t, Some(&labels())).as_deref(),
            Some("Ganztägig · 24. Juni bis 26. Juni"),
        );
    }

    #[test]
    fn without_labels_an_all_day_event_gets_no_body_rather_than_a_wrong_one() {
        // Before the frontend has pushed anything — the first seconds after a
        // launch. Saying less beats saying "00:00".
        let t = trigger(
            true,
            "2026-06-24T00:00:00Z",
            "2026-06-25T00:00:00Z",
            "00:00",
        );
        assert_eq!(notification_body(&t, None), None);
    }

    #[test]
    fn a_short_month_list_falls_back_to_the_plain_line() {
        // A frontend that pushed something unexpected. The range needs twelve
        // names; anything else answers the line that needs none, rather than
        // an empty month or a panic.
        let mut broken = labels();
        broken.months.truncate(5);
        let t = trigger(
            true,
            "2026-06-24T00:00:00Z",
            "2026-06-27T00:00:00Z",
            "00:00",
        );
        assert_eq!(
            notification_body(&t, Some(&broken)).as_deref(),
            Some("Ganztägig"),
        );
    }

    #[test]
    fn the_day_month_order_comes_from_the_template() {
        // Not a detail: "24. Juni" and "June 24" are the same date in two
        // orders, and deriving that here would mean the host deciding
        // something the frontend already knows.
        let english = ReminderLabels {
            all_day: "All day".into(),
            all_day_range: "All day · {{from}} to {{to}}".into(),
            day_month: "{{month}} {{day}}".into(),
            months: labels()
                .months
                .iter()
                .zip([
                    "January",
                    "February",
                    "March",
                    "April",
                    "May",
                    "June",
                    "July",
                    "August",
                    "September",
                    "October",
                    "November",
                    "December",
                ])
                .map(|(_, en)| en.to_string())
                .collect(),
        };
        let t = trigger(
            true,
            "2026-06-24T00:00:00Z",
            "2026-06-27T00:00:00Z",
            "00:00",
        );
        assert_eq!(
            notification_body(&t, Some(&english)).as_deref(),
            Some("All day · June 24 to June 26"),
        );
    }
}
