//! Reminder scheduler (DESIGN.md §14) — the Tauri delivery shell.
//!
//! The trigger ENUMERATION (which reminders fire, and when) now lives in
//! [`host_core::reminders`], shared with the mobile cal-ffi reminder surface.
//! This module keeps the desktop-only delivery:
//!   1. a single background tokio task that sleeps until the next trigger;
//!   2. dispatch through [`crate::toast`] (+ custom-sound playback), whose
//!      toast opens the reminder's item on a click;
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use adapter_local::{LocalAdapter, SharedConn};
use cal_core::SoundSource;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::Notify;
use tracing::{info, warn};

use crate::audio::AudioPlayer;
use crate::cache::CacheStore;
use crate::notification_shortcut::{NotificationShortcutState, HINT_EVENT};
use crate::os_notification::reminder_sound_name;
use crate::registry::AdapterRegistry;
use crate::toast::{show_toast, ToastRequest};
// Trigger enumeration + the shared types it produces. Re-exported so existing
// `crate::reminders::{Trigger, ItemKind, UpcomingReminder}` references (and the
// `get_upcoming_reminders` command) keep resolving.
use host_core::reminders::{
    catchup_eligible, enumerate_app_start_triggers, enumerate_birthday_triggers,
    enumerate_external_triggers, enumerate_local_triggers, external_calendar_triggers,
    external_task_list_triggers, mark_app_start_rung, ContainerTriggers, CATCH_UP_GRACE,
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
/// mutations don't change external state. It is read again only when the
/// worker wakes: the worker sleeps until its next reminder, so the TTL alone
/// does not bring a remote change in. What does is [`WrittenContainers`]:
/// Aperio's own writes, and the external refresh's reports of what changed at
/// the provider (`external_container_changed`), each read again alone.
struct ExternalTriggerCache {
    fetched_at: Instant,
    triggers: Vec<Trigger>,
}

impl ExternalTriggerCache {
    /// Put what was read again for one container in place of what the cache
    /// holds for it. A calendar and a task list may share an id (a CalDAV
    /// collection that holds both), so the kind decides with it. `None` (the
    /// read failed) keeps what the cache holds: the next full scan settles it.
    fn replace(&mut self, kind: ItemKind, container_id: &str, fresh: Option<Vec<Trigger>>) {
        let Some(fresh) = fresh else {
            return;
        };
        self.triggers
            .retain(|t| !(t.item_kind == kind && t.container_id == container_id));
        self.triggers.extend(fresh);
    }

    /// Put the birthday calendars' reminders, read again, in place of the
    /// ones the cache holds. They are made from the contacts, never read from
    /// a server, so they are read whole and cheaply.
    fn replace_birthdays(&mut self, fresh: Vec<Trigger>) {
        self.triggers
            .retain(|t| !host_core::birthdays::is_birthday_calendar_id(&t.container_id));
        self.triggers.extend(fresh);
    }
}

/// External containers that changed since the cache was read: Aperio wrote
/// to them, or the external refresh reported them changed at the provider.
/// The next scan reads them again, alone, instead of waiting for the cache to
/// run out: a reminder set or moved a moment ago would otherwise ring late,
/// and one of an occurrence deleted a moment ago would still ring. A removed
/// container is not read at all; its reminders simply go.
#[derive(Debug, Default, Clone)]
struct WrittenContainers {
    calendars: HashSet<String>,
    task_lists: HashSet<String>,
    removed_calendars: HashSet<String>,
    removed_task_lists: HashSet<String>,
    /// Contacts changed: the birthday calendars' reminders are made again.
    birthdays: bool,
    /// What a container-by-container read cannot answer (an account's list
    /// of calendars changed): the next scan reads everything.
    everything: bool,
}

impl WrittenContainers {
    fn is_empty(&self) -> bool {
        self.calendars.is_empty()
            && self.task_lists.is_empty()
            && self.removed_calendars.is_empty()
            && self.removed_task_lists.is_empty()
            && !self.birthdays
            && !self.everything
    }
}

/// What a report of the external refresh (`cache-updated`, one per container
/// whose content changed at the provider) means for the reminders.
#[derive(Debug, PartialEq, Eq)]
enum ExternalChange {
    Calendar,
    TaskList,
    Birthdays,
    Everything,
    Nothing,
}

/// [`ExternalChange`] by the report's scope (`SyncScope::as_str`).
fn external_change(scope: &str) -> ExternalChange {
    match scope {
        "events" => ExternalChange::Calendar,
        "tasks" => ExternalChange::TaskList,
        // A birthday calendar is made from a contact list's birthdays.
        "contacts" | "contact_lists" => ExternalChange::Birthdays,
        // A calendar or list came or went at the provider.
        "calendars" | "task_lists" => ExternalChange::Everything,
        _ => ExternalChange::Nothing,
    }
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
    /// What was written since `external_cache` was read (see
    /// [`WrittenContainers`]).
    external_written: Arc<Mutex<WrittenContainers>>,
    /// Counts the changes no read that began before them may answer for: the
    /// accounts with an adapter changed, or the cache was dropped. A read
    /// that saw another count when it began returns what it read but does
    /// not store it — the overview's scan, begun before an account was
    /// deleted, would otherwise put that account's reminders back.
    external_generation: Arc<AtomicU64>,
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
            external_written: Arc::new(Mutex::new(WrittenContainers::default())),
            external_generation: Arc::new(AtomicU64::new(0)),
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

    /// The one refresh interval (Einstellungen → Allgemein): how long the
    /// external snapshot answers, and how long the worker sleeps when nothing
    /// is planned. A change wakes the worker (`interval_changed`).
    fn refresh_interval(&self) -> Duration {
        host_core::user_prefs::sync_interval(&self.db)
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
        self.external_generation.fetch_add(1, Ordering::SeqCst);
        *guard = None;
    }

    /// An external calendar was written to (an event created, changed, moved
    /// in or out, deleted, or one occurrence skipped). The next scan reads its
    /// reminders again, that calendar alone, rather than when the cache runs
    /// out. Pairs with `invalidate()`, which starts that scan.
    pub fn external_calendar_written(&self, calendar_id: &str) {
        self.written().calendars.insert(calendar_id.to_string());
    }

    /// [`Self::external_calendar_written`] for an external task list.
    pub fn external_task_list_written(&self, list_id: &str) {
        self.written().task_lists.insert(list_id.to_string());
    }

    /// An external task list was deleted: its reminders go at the next scan,
    /// without a read of a list that is no longer there.
    pub fn external_task_list_removed(&self, list_id: &str) {
        self.written()
            .removed_task_lists
            .insert(list_id.to_string());
    }

    /// [`Self::external_task_list_removed`] for a calendar: a CalDAV
    /// collection deleted as a list was a calendar too.
    pub fn external_calendar_removed(&self, calendar_id: &str) {
        self.written()
            .removed_calendars
            .insert(calendar_id.to_string());
    }

    /// The external refresh found a container changed at the provider (an
    /// appointment a colleague added, a task done on the phone): its reminders
    /// are read again at once, rather than only when the cache runs out while
    /// the worker happens to be awake. Wakes the worker.
    pub fn external_container_changed(&self, scope: &str, container_id: &str) {
        {
            let mut written = self.written();
            match external_change(scope) {
                ExternalChange::Calendar => {
                    written.calendars.insert(container_id.to_string());
                }
                ExternalChange::TaskList => {
                    written.task_lists.insert(container_id.to_string());
                }
                ExternalChange::Birthdays => written.birthdays = true,
                ExternalChange::Everything => written.everything = true,
                ExternalChange::Nothing => return,
            }
        }
        self.invalidate();
    }

    /// The set of accounts with an adapter changed: one deleted (here or on
    /// another device), a plugin switched off, on, removed or replaced. The
    /// next scan reads every account again, without the ones that are gone,
    /// rather than ringing what was planned for them. Wakes the worker, which
    /// otherwise sleeps until the next planned reminder — possibly one of
    /// the deleted account's.
    pub fn accounts_changed(&self) {
        self.external_generation.fetch_add(1, Ordering::SeqCst);
        self.written().everything = true;
        self.invalidate();
    }

    /// A contact was created, changed or deleted, or an address book was
    /// deleted: the birthday reminders are read again at the next scan, which
    /// this starts. Local contacts change no cache the refresh would report,
    /// so nothing else would tell the planner.
    pub fn contacts_changed(&self) {
        self.written().birthdays = true;
        self.invalidate();
    }

    fn written(&self) -> std::sync::MutexGuard<'_, WrittenContainers> {
        self.external_written
            .lock()
            .expect("external written poison")
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
        // The overview reads; only the worker takes what changed.
        let mut triggers = self
            .collect_triggers_in_window(earliest, latest, false)
            .await;
        let hidden = self.hidden_calendars_snapshot();
        triggers.retain(|t| !Self::suppressed_by_visibility(t, &hidden));
        triggers.sort_by_key(|t| t.trigger_at);
        triggers
            .into_iter()
            .take(limit)
            .map(|t| UpcomingReminder {
                item_id: t.item_id,
                item_kind: t.item_kind,
                container_id: t.container_id,
                title: t.title,
                trigger_at: t.trigger_at.to_rfc3339(),
                start: t.start.to_rfc3339(),
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
                        _ = tokio::time::sleep(self.refresh_interval()) => continue,
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
        take_changes: bool,
    ) -> Vec<Trigger> {
        let mut acc = enumerate_local_triggers(&self.db, earliest, latest);
        let external = self.external_triggers_cached_or_fetch(take_changes).await;
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
    ///
    /// Only the worker TAKES what changed (`take_changes`); the overview
    /// (`upcoming`) reads it again for itself and leaves it, else the worker
    /// could plan from a cache the overview's read has not landed in yet, and
    /// nothing would wake it again.
    async fn external_triggers_cached_or_fetch(&self, take_changes: bool) -> Vec<Trigger> {
        // Taken before the cache is read: a write after this point is marked
        // again and read at the next scan, which its `invalidate()` starts.
        let written = if take_changes {
            std::mem::take(&mut *self.written())
        } else {
            self.written().clone()
        };
        // Read before the lock: it reads the database.
        let ttl = self.refresh_interval();
        let cache_fresh = {
            let guard = self.external_cache.lock().expect("external cache poison");
            match guard.as_ref() {
                Some(cached) if cached.fetched_at.elapsed() < ttl => {
                    if written.is_empty() {
                        return cached.triggers.clone();
                    }
                    true
                }
                _ => false,
            }
        };
        if cache_fresh {
            if let Some(triggers) = self.refresh_written(written).await {
                return triggers;
            }
        }
        self.fetch_all_external_triggers().await
    }

    /// Read again only the containers that changed since the cache was read,
    /// and put them into it. `None` when only a full scan can answer: the
    /// cache went meanwhile (a settings change dropped it), an account's list
    /// of calendars changed, or a container is shared by two accounts.
    async fn refresh_written(&self, written: WrittenContainers) -> Option<Vec<Trigger>> {
        if written.everything {
            return None;
        }
        let generation = self.external_generation.load(Ordering::SeqCst);
        let mut read = Vec::new();
        for id in written.calendars {
            let fresh = match external_calendar_triggers(&self.registry, &self.db, &id).await {
                ContainerTriggers::Read(triggers) => Some(triggers),
                ContainerTriggers::Unread => None,
                ContainerTriggers::Shared => return None,
            };
            read.push((ItemKind::Event, id, fresh));
        }
        for id in written.task_lists {
            let fresh = match external_task_list_triggers(&self.registry, &self.db, &id).await {
                ContainerTriggers::Read(triggers) => Some(triggers),
                ContainerTriggers::Unread => None,
                ContainerTriggers::Shared => return None,
            };
            read.push((ItemKind::Task, id, fresh));
        }
        for id in written.removed_calendars {
            read.push((ItemKind::Event, id, Some(Vec::new())));
        }
        for id in written.removed_task_lists {
            read.push((ItemKind::Task, id, Some(Vec::new())));
        }
        let birthdays = if written.birthdays {
            let local = LocalAdapter::new(self.db.clone());
            Some(enumerate_birthday_triggers(&local, &self.registry, &self.cache, &self.db).await)
        } else {
            None
        };
        let mut guard = self.external_cache.lock().expect("external cache poison");
        if self.external_generation.load(Ordering::SeqCst) != generation {
            // The accounts changed while this read: only a full read answers.
            return None;
        }
        let cached = guard.as_mut()?;
        for (kind, id, fresh) in read {
            cached.replace(kind, &id, fresh);
        }
        if let Some(fresh) = birthdays {
            cached.replace_birthdays(fresh);
        }
        Some(cached.triggers.clone())
    }

    /// The full external scan, every account, every calendar and list, into
    /// the cache.
    async fn fetch_all_external_triggers(&self) -> Vec<Trigger> {
        let generation = self.external_generation.load(Ordering::SeqCst);
        let mut fresh = enumerate_external_triggers(&self.registry, &self.db).await;
        // Synthetic birthday calendars aren't adapters, so the external fan-out
        // above skips them. Fold their configured default reminders in here so
        // they share the same TTL cache (and the same
        // `invalidate_external_cache` after a "Standard-Hinweise" change).
        let local = LocalAdapter::new(self.db.clone());
        fresh.extend(
            enumerate_birthday_triggers(&local, &self.registry, &self.cache, &self.db).await,
        );
        self.store_full_read(generation, &fresh);
        fresh
    }

    /// Keep a full read as the cache, unless the accounts changed (or the
    /// cache was dropped) since it began: then it may hold an account that
    /// is gone, and the read their change asked for is the one to keep.
    /// Returns whether it was kept.
    fn store_full_read(&self, began_at: u64, fresh: &[Trigger]) -> bool {
        let mut guard = self.external_cache.lock().expect("external cache poison");
        if self.external_generation.load(Ordering::SeqCst) != began_at {
            return false;
        }
        *guard = Some(ExternalTriggerCache {
            fetched_at: Instant::now(),
            triggers: fresh.to_vec(),
        });
        true
    }

    /// Build the list of pending (not-yet-fired, in-the-future or catch-up-
    /// eligible) reminder triggers for the scheduler. Lookahead is bounded by
    /// `MAX_HORIZON_DAYS`; the past bound reaches `CATCH_UP_HORIZON` so reminders
    /// that lapsed while the app was closed still make it into the scan (the
    /// catch-up filter then keeps a past one only while the later of its event's
    /// end and its own moment is at most `CATCH_UP_GRACE` ago).
    async fn collect_pending_triggers(&self) -> Vec<Trigger> {
        let now = Utc::now();
        let earliest = now - ChronoDuration::days(CATCH_UP_HORIZON);
        let latest = now + ChronoDuration::days(MAX_HORIZON_DAYS);
        let mut out = self
            .collect_triggers_in_window(earliest, latest, true)
            .await;

        // De-duplicate (same item_id + same trigger time appearing from both
        // local SQLite and an external adapter — keep the first occurrence).
        // Of twins at one moment (a fixed-time reminder on a series rings from
        // every occurrence) the first that has not ended when it rings comes
        // first, as on the phone (`reminder_dtos`); the sort is stable, so
        // local still comes before external.
        out.sort_by_key(|t| (t.trigger_at, t.relevant_until <= t.trigger_at));
        let mut seen: HashSet<(String, String)> = HashSet::new();
        out.retain(|t| seen.insert((t.item_id.clone(), t.trigger_at.to_rfc3339())));

        // Catch-up filter. Future triggers always stay (the worker schedules
        // them via tokio::sleep). Past triggers stay only when the event itself
        // is still relevant, or the reminder itself is fresh — see
        // `catchup_eligible`.
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
        // §14.4 playback dispatch: a custom sound Aperio plays itself (a
        // toast can't), and the toast then asks for no sound, so nothing
        // plays twice. What the toast asks for is
        // `os_notification::reminder_sound_name`: the system sound for
        // System and for a custom sound whose file is missing, none for
        // Silent.
        let custom_file_found = match &t.sound.source {
            SoundSource::Custom { sha256 } => {
                match crate::sound_assets::local_sound_path(&self.sounds_dir, sha256) {
                    Some(path) => {
                        self.audio.play_file(path);
                        true
                    }
                    None => {
                        warn!(
                            hash = %sha256,
                            "custom sound file missing; falling back to system sound",
                        );
                        false
                    }
                }
            }
            SoundSource::System | SoundSource::Silent => false,
        };
        // A click opens what the reminder was for (TODO A5, `toast`).
        let tap = serde_json::to_value(t.tap_data()).ok();
        show_toast(
            app,
            ToastRequest {
                title: t.title.clone(),
                body: self.notification_body(t),
                sound: reminder_sound_name(&t.sound.source, custom_file_found),
                tap,
            },
        );
        // Windows stores this toast and shows nothing while Aperio has no
        // Start menu shortcut for notifications: the first such reminder of
        // an app start asks the page to offer one (`notification_shortcut`).
        if let Some(shortcut) = app.try_state::<Arc<NotificationShortcutState>>() {
            if shortcut.note_reminder_fired() {
                let _ = app.emit(HINT_EVENT, ());
            }
        }
        let mut fired = self.fired.lock().expect("fired set poisoned");
        fired.insert(FiredKey {
            item_id: t.item_id.clone(),
            trigger_iso: t.trigger_at.to_rfc3339(),
        });
    }

    /// Fire the `app_start` reminders due at this start, and remember them so
    /// the next start does not ring them again. The scan and the memory live in
    /// [`host_core::reminders`]; this dispatches each.
    fn fire_app_start_reminders<R: Runtime>(&self, app: &AppHandle<R>) {
        // Empty until the frontend's first push, so a hidden calendar's
        // (local-only) app_start reminder may still fire once on a cold launch
        // before the push lands; the periodic loop suppresses it thereafter.
        // A suppressed one is not remembered: it rings at the first start
        // after its calendar is shown again.
        let hidden = self.hidden_calendars_snapshot();
        let mut rung = Vec::new();
        for t in enumerate_app_start_triggers(&self.db) {
            if Self::suppressed_by_visibility(&t, &hidden) {
                continue;
            }
            self.fire(app, &t);
            rung.push(t);
        }
        mark_app_start_rung(&self.db, &rung);
    }
}

/// How many days into the future the scheduler looks ahead in a single pass. A
/// new mutation invalidates the loop, so the horizon only needs to bridge a
/// quiet stretch.
const MAX_HORIZON_DAYS: i64 = 30;
/// How far back the scheduler reaches for catch-up candidates — reminders whose
/// trigger time lapsed while the app was closed. The catch-up filter drops
/// anything whose event and own moment both lie more than `CATCH_UP_GRACE`
/// back, so this is a scan-size guard.
const CATCH_UP_HORIZON: i64 = 7;
/// How far back the Ctrl+Shift+R overview looks for already-passed reminders.
const OVERVIEW_PAST_DAYS: i64 = 7;
/// How far forward the overview shows upcoming reminders. Longer than the
/// scheduler horizon so the user can plan ahead.
const OVERVIEW_FUTURE_DAYS: i64 = 90;
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
            absolute: false,
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

    /// What was read again for one container replaces what the cache held
    /// for it, and nothing else.
    mod written_containers {
        use super::*;

        fn of(id: &str, kind: ItemKind, container: &str) -> Trigger {
            Trigger {
                item_id: id.into(),
                item_kind: kind,
                container_id: container.into(),
                ..trigger(
                    false,
                    "2026-11-09T09:00:00Z",
                    "2026-11-09T10:00:00Z",
                    "09:00",
                )
            }
        }

        fn ids(cache: &ExternalTriggerCache) -> Vec<String> {
            let mut ids: Vec<_> = cache.triggers.iter().map(|t| t.item_id.clone()).collect();
            ids.sort();
            ids
        }

        fn cache() -> ExternalTriggerCache {
            ExternalTriggerCache {
                fetched_at: Instant::now(),
                triggers: vec![
                    of("old-event", ItemKind::Event, "shared-id"),
                    of("old-task", ItemKind::Task, "shared-id"),
                    of("other", ItemKind::Event, "cal-2"),
                ],
            }
        }

        #[test]
        fn a_calendar_read_again_replaces_its_own_reminders_only() {
            let mut cache = cache();
            cache.replace(
                ItemKind::Event,
                "shared-id",
                Some(vec![of("new-event", ItemKind::Event, "shared-id")]),
            );
            // A task list with the same id (a CalDAV collection holding both)
            // keeps its reminders; another calendar keeps its own.
            assert_eq!(ids(&cache), vec!["new-event", "old-task", "other"]);
        }

        #[test]
        fn a_list_read_again_as_empty_loses_its_reminders() {
            let mut cache = cache();
            cache.replace(ItemKind::Task, "shared-id", Some(Vec::new()));
            assert_eq!(ids(&cache), vec!["old-event", "other"]);
        }

        #[test]
        fn a_failed_read_keeps_what_the_cache_holds() {
            let mut cache = cache();
            cache.replace(ItemKind::Event, "shared-id", None);
            assert_eq!(ids(&cache), vec!["old-event", "old-task", "other"]);
        }

        /// One account with calendar `cal-1` and list `list-1`, both empty,
        /// counting what it is asked.
        #[derive(Default)]
        struct Counting {
            calendar_listings: std::sync::atomic::AtomicU32,
            list_listings: std::sync::atomic::AtomicU32,
            reads: Mutex<Vec<String>>,
        }

        impl Counting {
            fn listings(&self) -> u32 {
                use std::sync::atomic::Ordering::SeqCst;
                self.calendar_listings.load(SeqCst) + self.list_listings.load(SeqCst)
            }
            fn reads(&self) -> Vec<String> {
                self.reads.lock().unwrap().clone()
            }
        }

        #[async_trait::async_trait]
        impl cal_core::Adapter for Counting {
            async fn authenticate(
                &self,
                _: cal_core::Credentials,
            ) -> cal_core::Result<cal_core::AuthToken> {
                unreachable!()
            }
            fn capabilities(&self) -> &[cal_core::Capability] {
                &[]
            }
        }

        #[async_trait::async_trait]
        impl cal_core::CalendarFeature for Counting {
            async fn list_calendars(&self) -> cal_core::Result<Vec<cal_core::Calendar>> {
                self.calendar_listings
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::new())
            }
            async fn get_events(
                &self,
                calendar_id: &str,
                _: cal_core::DateRange,
            ) -> cal_core::Result<Vec<cal_core::Event>> {
                self.reads
                    .lock()
                    .unwrap()
                    .push(format!("events {calendar_id}"));
                Ok(Vec::new())
            }
            async fn create_event(
                &self,
                _: &str,
                _: cal_core::NewEvent,
            ) -> cal_core::Result<cal_core::Event> {
                unreachable!()
            }
            async fn update_event(&self, _: cal_core::Event) -> cal_core::Result<cal_core::Event> {
                unreachable!()
            }
            async fn delete_event(&self, _: &str, _: bool) -> cal_core::Result<()> {
                unreachable!()
            }
            async fn get_free_busy(
                &self,
                _: &[&str],
                _: cal_core::DateRange,
            ) -> cal_core::Result<Vec<cal_core::FreeBusy>> {
                unreachable!()
            }
            fn calendar_color(&self, _: &str) -> Option<cal_core::ContainerColor> {
                None
            }
        }

        #[async_trait::async_trait]
        impl cal_core::TasksFeature for Counting {
            async fn list_task_lists(&self) -> cal_core::Result<Vec<cal_core::TaskList>> {
                self.list_listings
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(Vec::new())
            }
            async fn get_tasks(&self, list_id: &str) -> cal_core::Result<Vec<cal_core::Task>> {
                self.reads.lock().unwrap().push(format!("tasks {list_id}"));
                Ok(Vec::new())
            }
            async fn create_task(
                &self,
                _: &str,
                _: cal_core::NewTask,
            ) -> cal_core::Result<cal_core::Task> {
                unreachable!()
            }
            async fn update_task(&self, _: cal_core::Task) -> cal_core::Result<cal_core::Task> {
                unreachable!()
            }
            async fn delete_task(&self, _: &str) -> cal_core::Result<()> {
                unreachable!()
            }
        }

        /// A scheduler whose external cache holds `cached`, read `age` ago,
        /// over an account that answers through `account`.
        fn scheduler(
            account: Arc<Counting>,
            cached: Vec<Trigger>,
            age: Duration,
        ) -> ReminderScheduler {
            let db = host_core::db::DbHandle::open_in_memory().expect("in-memory db");
            let registry = Arc::new(AdapterRegistry::new(
                Arc::new(plugin_core::PluginManager::new("0.1.0")),
                Arc::new(sync_engine::test_support::FakeSecrets::default()),
            ));
            registry.register_host_adapter(
                "acc-1",
                Some(account.clone() as Arc<dyn cal_core::CalendarFeature>),
                Some(account as Arc<dyn cal_core::TasksFeature>),
            );
            registry.note_calendar_route("cal-1", "acc-1");
            registry.note_task_list_route("list-1", "acc-1");
            let fetched_at = Instant::now()
                .checked_sub(age)
                .expect("the clock reaches back that far");
            ReminderScheduler {
                db: db.shared(),
                registry,
                cache: Arc::new(CacheStore::new(db)),
                invalidate: Arc::new(Notify::new()),
                fired: Arc::new(Mutex::new(HashSet::new())),
                external_cache: Arc::new(Mutex::new(Some(ExternalTriggerCache {
                    fetched_at,
                    triggers: cached,
                }))),
                external_written: Arc::new(Mutex::new(WrittenContainers::default())),
                external_generation: Arc::new(AtomicU64::new(0)),
                hidden_calendars: Arc::new(Mutex::new(HashSet::new())),
                labels: Arc::new(Mutex::new(None)),
                sounds_dir: PathBuf::new(),
                audio: crate::audio::AudioPlayer::silent(),
            }
        }

        fn sorted(triggers: Vec<Trigger>) -> Vec<String> {
            let mut ids: Vec<_> = triggers.into_iter().map(|t| t.item_id).collect();
            ids.sort();
            ids
        }

        #[tokio::test]
        async fn a_written_calendar_is_read_again_alone_and_the_rest_stays() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![
                    // An occurrence deleted a moment ago, still in the cache.
                    of("deleted", ItemKind::Event, "cal-1"),
                    of("other", ItemKind::Event, "cal-2"),
                    of("task", ItemKind::Task, "list-1"),
                ],
                Duration::from_secs(10),
            );
            // Untouched, the cache answers and nothing is read.
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["deleted", "other", "task"]
            );
            assert!(account.reads().is_empty());

            s.external_calendar_written("cal-1");
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["other", "task"]
            );
            assert_eq!(account.reads(), vec!["events cal-1".to_string()]);
            assert_eq!(account.listings(), 0, "no scan of every account");

            // Read once: the next scan answers from the cache again.
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["other", "task"]
            );
            assert_eq!(account.reads().len(), 1);
        }

        #[tokio::test]
        async fn a_written_list_is_read_again_and_a_removed_one_is_not_read() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![
                    of("t1", ItemKind::Task, "list-1"),
                    of("t2", ItemKind::Task, "list-2"),
                    of("e", ItemKind::Event, "cal-1"),
                ],
                Duration::from_secs(10),
            );
            s.external_task_list_written("list-1");
            s.external_task_list_removed("list-2");
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["e"]
            );
            assert_eq!(account.reads(), vec!["tasks list-1".to_string()]);
            assert_eq!(account.listings(), 0);
        }

        /// The planner's snapshot answers as long as the refresh interval
        /// the user chose, not a fixed five minutes.
        #[tokio::test]
        async fn the_external_snapshot_lives_as_long_as_the_refresh_interval() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![of("kept", ItemKind::Event, "cal-1")],
                Duration::from_secs(6 * 60),
            );
            host_core::user_prefs::UserPrefsRepo::new(&s.db)
                .set(sync_engine::PREF_SYNC_INTERVAL_MINUTES, "15")
                .unwrap();
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["kept"]
            );
            assert_eq!(account.listings(), 0, "six minutes old is fresh at 15");
        }

        #[tokio::test]
        async fn a_cache_that_ran_out_is_read_whole_as_before() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![of("old", ItemKind::Event, "cal-9")],
                host_core::user_prefs::sync_interval(
                    &host_core::db::DbHandle::open_in_memory().unwrap().shared(),
                ) + Duration::from_secs(1),
            );
            s.external_calendar_written("cal-1");
            assert!(s.external_triggers_cached_or_fetch(true).await.is_empty());
            // The full scan lists every account's calendars and lists.
            assert_eq!(account.listings(), 2);
        }

        #[tokio::test]
        async fn the_overview_reads_what_changed_and_leaves_it_to_the_worker() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![of("deleted", ItemKind::Event, "cal-1")],
                Duration::from_secs(10),
            );
            s.external_calendar_written("cal-1");
            // Ctrl+Shift+R reads the calendar again for itself...
            assert!(s.external_triggers_cached_or_fetch(false).await.is_empty());
            assert_eq!(account.reads().len(), 1);
            // ...and the worker still finds it marked, so it plans from a read
            // of its own, not from one that may not have landed yet.
            assert!(s.external_triggers_cached_or_fetch(true).await.is_empty());
            assert_eq!(account.reads().len(), 2);
            assert!(s.external_triggers_cached_or_fetch(true).await.is_empty());
            assert_eq!(account.reads().len(), 2, "taken once");
        }

        #[tokio::test]
        async fn a_container_two_accounts_list_is_left_to_the_full_scan() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![of("old", ItemKind::Event, "cal-1")],
                Duration::from_secs(10),
            );
            {
                let conn = s.db.lock().unwrap();
                for id in ["acc-1", "acc-2"] {
                    conn.execute(
                        "INSERT INTO accounts (id, display_name, adapter_kind, config_json, created_at, updated_at) \
                         VALUES (?1, ?1, 'google', '{}', '2026-11-09T00:00:00Z', '2026-11-09T00:00:00Z')",
                        [id],
                    )
                    .unwrap();
                    conn.execute(
                        "INSERT INTO cache_calendars (account_id, id, payload, cached_at) \
                         VALUES (?1, 'cal-1', '{}', '2026-11-09T00:00:00Z')",
                        [id],
                    )
                    .unwrap();
                }
            }
            s.external_calendar_written("cal-1");
            assert!(s.external_triggers_cached_or_fetch(true).await.is_empty());
            // Each account has its own reminders on it: read whole.
            assert_eq!(account.listings(), 2);
        }

        #[tokio::test]
        async fn a_change_at_the_provider_reads_its_container_again_and_wakes_the_worker() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![
                    of("gone", ItemKind::Event, "cal-1"),
                    of("other", ItemKind::Event, "cal-2"),
                ],
                Duration::from_secs(10),
            );
            s.external_container_changed("events", "cal-1");
            assert!(
                tokio::time::timeout(Duration::from_millis(50), s.invalidate.notified())
                    .await
                    .is_ok(),
                "the worker is woken"
            );
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["other"]
            );
            assert_eq!(account.reads(), vec!["events cal-1".to_string()]);
            assert_eq!(account.listings(), 0);
        }

        #[tokio::test]
        async fn changed_contacts_make_the_birthdays_again_without_a_server() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![
                    of("birthday", ItemKind::Event, "aperio-birthdays:list-1"),
                    of("other", ItemKind::Event, "cal-2"),
                ],
                Duration::from_secs(10),
            );
            s.external_container_changed("contacts", "list-1");
            // No contact has a birthday in this store: the old one goes.
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["other"]
            );
            assert!(account.reads().is_empty());
            assert_eq!(account.listings(), 0);
        }

        #[tokio::test]
        async fn a_changed_list_of_calendars_reads_everything() {
            let account = Arc::new(Counting::default());
            let s = scheduler(account.clone(), Vec::new(), Duration::from_secs(10));
            s.external_container_changed("calendars", "");
            s.external_triggers_cached_or_fetch(true).await;
            assert_eq!(account.listings(), 2);
        }

        #[tokio::test]
        async fn a_removed_calendar_loses_its_reminders_without_a_read() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![
                    of("e", ItemKind::Event, "both"),
                    of("t", ItemKind::Task, "both"),
                ],
                Duration::from_secs(10),
            );
            s.external_calendar_removed("both");
            s.external_task_list_removed("both");
            assert!(s.external_triggers_cached_or_fetch(true).await.is_empty());
            assert!(account.reads().is_empty());
        }

        /// A deleted account's reminders go at the next scan, and the worker
        /// is woken for it instead of sleeping until one of them.
        #[tokio::test]
        async fn a_deleted_account_takes_its_reminders_along() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![of("gone", ItemKind::Event, "cal-1")],
                Duration::from_secs(10),
            );
            s.registry.unregister("acc-1");
            s.accounts_changed();
            tokio::time::timeout(Duration::from_secs(1), s.invalidate.notified())
                .await
                .expect("the worker is woken");
            assert!(s.external_triggers_cached_or_fetch(true).await.is_empty());
            assert_eq!(account.listings(), 0, "the deleted account is not asked");
        }

        /// A full read that began before the accounts changed is returned
        /// but not kept: it may hold the account that went.
        #[tokio::test]
        async fn a_read_begun_before_an_account_change_is_not_kept() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![of("kept", ItemKind::Event, "cal-2")],
                Duration::from_secs(10),
            );
            let began = s.external_generation.load(Ordering::SeqCst);
            s.accounts_changed();
            assert!(!s.store_full_read(began, &[of("gone", ItemKind::Event, "cal-1")]));
            let cached = s.external_cache.lock().unwrap();
            let ids: Vec<_> = cached
                .as_ref()
                .expect("the cache stays")
                .triggers
                .iter()
                .map(|t| t.item_id.clone())
                .collect();
            assert_eq!(ids, vec!["kept".to_string()]);
            drop(cached);
            let now = s.external_generation.load(Ordering::SeqCst);
            assert!(s.store_full_read(now, &[of("fresh", ItemKind::Event, "cal-1")]));
        }

        /// A contact change reads the birthdays again at once (no refresh
        /// reports a local contact), and wakes the worker for it.
        #[tokio::test]
        async fn a_contact_change_reads_the_birthdays_again() {
            let account = Arc::new(Counting::default());
            let s = scheduler(
                account.clone(),
                vec![
                    of("birthday", ItemKind::Event, "aperio-birthdays:list-1"),
                    of("other", ItemKind::Event, "cal-2"),
                ],
                Duration::from_secs(10),
            );
            s.contacts_changed();
            tokio::time::timeout(Duration::from_secs(1), s.invalidate.notified())
                .await
                .expect("the worker is woken");
            assert_eq!(
                sorted(s.external_triggers_cached_or_fetch(true).await),
                vec!["other"]
            );
            assert!(account.reads().is_empty());
        }

        #[test]
        fn a_report_names_what_it_means_for_the_reminders() {
            assert_eq!(external_change("events"), ExternalChange::Calendar);
            assert_eq!(external_change("tasks"), ExternalChange::TaskList);
            assert_eq!(external_change("contacts"), ExternalChange::Birthdays);
            assert_eq!(external_change("contact_lists"), ExternalChange::Birthdays);
            assert_eq!(external_change("calendars"), ExternalChange::Everything);
            assert_eq!(external_change("task_lists"), ExternalChange::Everything);
            assert_eq!(external_change("sections"), ExternalChange::Nothing);
        }

        #[test]
        fn nothing_written_is_empty_and_anything_written_is_not() {
            assert!(WrittenContainers::default().is_empty());
            for written in [
                WrittenContainers {
                    calendars: HashSet::from(["c".to_string()]),
                    ..Default::default()
                },
                WrittenContainers {
                    task_lists: HashSet::from(["l".to_string()]),
                    ..Default::default()
                },
                WrittenContainers {
                    removed_task_lists: HashSet::from(["l".to_string()]),
                    ..Default::default()
                },
                WrittenContainers {
                    removed_calendars: HashSet::from(["c".to_string()]),
                    ..Default::default()
                },
                WrittenContainers {
                    birthdays: true,
                    ..Default::default()
                },
                WrittenContainers {
                    everything: true,
                    ..Default::default()
                },
            ] {
                assert!(!written.is_empty(), "{written:?}");
            }
        }
    }
}
