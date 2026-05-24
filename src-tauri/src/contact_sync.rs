//! Contact sync scheduler (DESIGN.md §10.5).
//!
//! Walks every external `ContactsFeature` adapter on three triggers:
//!
//!   1. **App start** — once, after a short delay so the rest of the
//!      app finishes booting and the registry has materialised every
//!      account.
//!   2. **Periodic** — every `contacts.syncIntervalMinutes` minutes
//!      (default 60). The interval is read from `user_prefs` on
//!      every loop so a settings change takes effect on the next
//!      tick without an app restart.
//!   3. **Manual** — the `sync_contacts_now` Tauri command + the
//!      "Refresh" button in the contacts view.
//!
//! A sync pass does **not** persist data into our own SQLite — each
//! adapter owns its own listing + per-list cache (5-minute TTL), and
//! the pass just calls `list_contact_lists` followed by
//! `get_contacts` on every writable list so those caches stay warm.
//! The frontend re-reads via the existing `get_contacts` command
//! after we emit `contacts-synced` and the cached data flows in
//! cheaply.
//!
//! ## What gets pulled per pass
//!
//! Only **writable** lists are pulled on app-start + periodic
//! passes. Read-only sentinels (the EWS GAL, Google Other Contacts,
//! Google Directory, the Graph Suggested People stream) carry
//! thousands of rows and are paid for only on explicit user action
//! (selecting that list in the sidebar). The manual `sync_contacts_now`
//! command with `force_read_only = true` overrides that — useful as
//! the "force-pull everything" shortcut in Settings.
//!
//! ## In-flight dedupe
//!
//! A `Mutex<bool>` guards against overlapping passes — clicking
//! Refresh ten times triggers exactly one walk. Subsequent calls
//! return immediately with `false`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};
use tokio::sync::Notify;
use tracing::{debug, info, warn};

use crate::db::SharedConn;
use crate::registry::AdapterRegistry;
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key for the configurable polling interval, in
/// minutes. Defaults to 60 — matches the value DESIGN.md §10.5
/// names as the standard cadence.
pub const PREF_SYNC_INTERVAL_MINUTES: &str = "contacts.syncIntervalMinutes";

/// `user_prefs` key for the timestamp of the most recent successful
/// sync pass, persisted as RFC 3339. Used by the contacts view's
/// "Last synced at …" footer so the value survives restarts.
pub const PREF_LAST_SYNCED_AT: &str = "contacts.lastSyncedAt";

/// `user_prefs` key for the "also pull read-only directories"
/// toggle (Settings → Kontakte). When `true`, every sync pass —
/// app-start, periodic, and manual — walks the expensive
/// read-only sentinels (EWS GAL, Google Other Contacts /
/// Workspace Directory, MS Graph Suggested People). Defaults to
/// `false` so a quiet account doesn't pay the multi-minute scan
/// cost without the user opting in.
///
/// Stored as the literal strings `"true"` / `"false"` (same
/// convention `sync.adapter.e2eEnabled` uses) so the keychain
/// debug view stays readable. The manual `sync_contacts_now`
/// command keeps an explicit override parameter that beats this
/// pref — used by background hooks that want a one-shot
/// directory pull without flipping the user-visible setting.
pub const PREF_INCLUDE_READ_ONLY_ON_SYNC: &str =
    "contacts.includeReadOnlyOnSync";

/// Default interval if `PREF_SYNC_INTERVAL_MINUTES` is unset or
/// nonsense. 60 minutes per the design doc.
pub const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 60;

/// How long after app start the first sync pass kicks off. Lets the
/// Tauri main thread finish wiring everything up + lets the UI
/// paint before we start consuming bandwidth.
const APP_START_DELAY: Duration = Duration::from_secs(5);

/// Frontend payload for the `contacts-synced` event the scheduler
/// emits after a successful pass. The frontend uses
/// `lastSyncedAt` to update the "Last synced at …" footer and to
/// invalidate its SWR cache so a subsequent `useContacts` refetch
/// picks up the freshly-warmed adapter cache.
#[derive(Debug, Clone, Serialize)]
pub struct ContactsSyncedPayload {
    pub last_synced_at: String,
    /// Account ids whose pass completed without an adapter-level
    /// error. The frontend uses this to decide which lists to
    /// refetch — accounts that errored keep their existing
    /// (possibly stale) data rather than flickering to empty.
    pub succeeded_accounts: Vec<String>,
    /// Account ids whose sync failed (any list under them threw).
    /// Surface a chip / icon for these in the panel so the user
    /// knows the data they see for those books may be stale.
    pub failed_accounts: Vec<String>,
}

/// Status snapshot read by the contacts view's footer. The values
/// here come straight from `user_prefs` — no in-memory state past
/// what the persisted timestamps tell us.
#[derive(Debug, Clone, Serialize)]
pub struct ContactsSyncStatus {
    pub last_synced_at: Option<String>,
    pub interval_minutes: u32,
    pub in_flight: bool,
    /// Current value of [`PREF_INCLUDE_READ_ONLY_ON_SYNC`]. The
    /// Settings → Kontakte checkbox seeds itself from this so the
    /// UI doesn't need a separate `get_user_pref` round-trip; the
    /// `useContactSync` polling cycle already keeps the rest of
    /// the status fresh.
    pub include_read_only_on_sync: bool,
}

pub struct ContactSyncScheduler {
    registry: Arc<AdapterRegistry>,
    db: SharedConn,
    /// Last successful sync time, kept in memory so the periodic
    /// loop can read it without touching SQLite. Persisted to
    /// `user_prefs` in the same atomic step so the value survives
    /// restarts.
    last_synced_at: Arc<Mutex<Option<DateTime<Utc>>>>,
    /// In-flight guard. `true` while a pass is running; further
    /// `run_sync` calls return early.
    in_flight: Arc<Mutex<bool>>,
    /// Lets `sync_contacts_now` wake the periodic loop out of its
    /// sleep so the next interval starts counting from the manual
    /// kick rather than the previous tick — mirrors the
    /// ReminderScheduler's invalidate pattern.
    notify: Arc<Notify>,
}

impl ContactSyncScheduler {
    /// Construct a scheduler + start its periodic worker. The
    /// returned `Arc` is shared between Tauri State (for the
    /// `sync_contacts_now` command) and the background worker
    /// itself.
    pub fn spawn<R: Runtime>(
        registry: Arc<AdapterRegistry>,
        db: SharedConn,
        app: AppHandle<R>,
    ) -> Arc<Self> {
        // Hydrate the in-memory last_synced_at from prefs so the
        // first `get_contacts_sync_status` after restart reflects
        // the persisted value without waiting for the first pass
        // to complete.
        let initial_last = {
            let repo = UserPrefsRepo::new(&db);
            repo.get(PREF_LAST_SYNCED_AT)
                .ok()
                .flatten()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc))
        };

        let scheduler = Arc::new(Self {
            registry,
            db,
            last_synced_at: Arc::new(Mutex::new(initial_last)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        });

        let worker = scheduler.clone();
        tauri::async_runtime::spawn(async move {
            // App-start kick — let the UI settle for 5 s before
            // pulling. The directory pull (GAL etc.) honours the
            // user's `contacts.includeReadOnlyOnSync` pref so a
            // user who explicitly opted in gets the GAL on boot
            // too. Default-off keeps the boot cheap for everyone
            // else.
            tokio::time::sleep(APP_START_DELAY).await;
            let include_ro = worker.read_include_read_only_on_sync();
            info!(include_ro, "running app-start contact sync pass");
            worker.run_sync(&app, include_ro).await;

            // Periodic loop. Re-read both the interval and the
            // include-read-only pref on every tick so a settings
            // change applies on the next pass; no restart needed.
            loop {
                let minutes = worker.read_interval_minutes();
                let dur = Duration::from_secs(u64::from(minutes) * 60);
                tokio::select! {
                    _ = tokio::time::sleep(dur) => {
                        let include_ro =
                            worker.read_include_read_only_on_sync();
                        info!(?dur, include_ro, "periodic contact sync tick");
                        worker.run_sync(&app, include_ro).await;
                    }
                    _ = worker.notify.notified() => {
                        // Manual sync just landed; restart the
                        // interval clock without firing another
                        // pass (the manual call already did one).
                        debug!("contact sync interval reset by manual trigger");
                    }
                }
            }
        });
        scheduler
    }

    /// Read the configured interval from `user_prefs`. Defaults to
    /// `DEFAULT_SYNC_INTERVAL_MINUTES` when missing or unparseable.
    /// Values below 1 minute clamp to 1 so a typo (`0`) doesn't pin
    /// the worker into a hot loop.
    pub fn read_interval_minutes(&self) -> u32 {
        let repo = UserPrefsRepo::new(&self.db);
        repo.get(PREF_SYNC_INTERVAL_MINUTES)
            .ok()
            .flatten()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|m| m.max(1))
            .unwrap_or(DEFAULT_SYNC_INTERVAL_MINUTES)
    }

    /// Read the current value of [`PREF_INCLUDE_READ_ONLY_ON_SYNC`].
    /// Anything other than the literal `"true"` (case-sensitive)
    /// reads as `false` — same lenient parse the scheduler uses
    /// for the E2E flag.
    ///
    /// Re-read on every periodic tick so a checkbox change in
    /// Settings applies on the next pass; no restart needed.
    pub fn read_include_read_only_on_sync(&self) -> bool {
        let repo = UserPrefsRepo::new(&self.db);
        repo.get(PREF_INCLUDE_READ_ONLY_ON_SYNC)
            .ok()
            .flatten()
            .as_deref()
            == Some("true")
    }

    /// Snapshot the current sync status — used by the
    /// `get_contacts_sync_status` Tauri command.
    pub fn status(&self) -> ContactsSyncStatus {
        let last = self.last_synced_at.lock().expect("sync mutex poisoned");
        let in_flight = *self.in_flight.lock().expect("sync in-flight poisoned");
        ContactsSyncStatus {
            last_synced_at: last.map(|d| d.to_rfc3339()),
            interval_minutes: self.read_interval_minutes(),
            in_flight,
            include_read_only_on_sync: self.read_include_read_only_on_sync(),
        }
    }

    /// Run one sync pass. `include_read_only` opts into pulling the
    /// expensive read-only sentinel lists (GAL etc.) — defaults to
    /// `false` on the auto-triggered paths and `true` only on a
    /// manual "force everything" command.
    ///
    /// Returns `true` when this call actually ran the pass, `false`
    /// when another pass was already in flight and this invocation
    /// was deduped.
    pub async fn run_sync<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        include_read_only: bool,
    ) -> bool {
        // Acquire the in-flight guard. Release-on-drop would be
        // cleaner but the lock is sync, the closure body is async,
        // and the guard would have to cross await points — so we
        // toggle the bool manually and clear it before every exit.
        {
            let mut guard = self.in_flight.lock().expect("sync in-flight poisoned");
            if *guard {
                debug!("contact sync skipped — another pass is already in flight");
                return false;
            }
            *guard = true;
        }

        let mut succeeded: Vec<String> = Vec::new();
        let mut failed: Vec<String> = Vec::new();

        for (account_id, adapter) in self.registry.snapshot_contact_adapters() {
            let lists = match adapter.list_contact_lists().await {
                Ok(lists) => lists,
                Err(err) => {
                    warn!(
                        account_id = %account_id,
                        ?err,
                        "list_contact_lists failed during sync pass",
                    );
                    failed.push(account_id);
                    continue;
                }
            };
            let mut account_failed = false;
            for list in lists {
                if list.read_only && !include_read_only {
                    // Skip the GAL / Suggested People / Other
                    // Contacts / Workspace Directory on auto
                    // passes — they're expensive to walk and the
                    // user opts in by selecting them in the
                    // sidebar.
                    continue;
                }
                if let Err(err) = adapter.get_contacts(&list.id).await {
                    warn!(
                        account_id = %account_id,
                        list_id = %list.id,
                        ?err,
                        "get_contacts failed during sync pass",
                    );
                    account_failed = true;
                }
            }
            if account_failed {
                failed.push(account_id);
            } else {
                succeeded.push(account_id);
            }
        }

        // Persist + broadcast.
        let now = Utc::now();
        {
            let mut guard = self.last_synced_at.lock().expect("sync mutex poisoned");
            *guard = Some(now);
        }
        let repo = UserPrefsRepo::new(&self.db);
        if let Err(err) = repo.set(PREF_LAST_SYNCED_AT, &now.to_rfc3339()) {
            warn!(?err, "failed to persist contacts.lastSyncedAt");
        }

        let payload = ContactsSyncedPayload {
            last_synced_at: now.to_rfc3339(),
            succeeded_accounts: succeeded,
            failed_accounts: failed,
        };
        if let Err(err) = app.emit("contacts-synced", &payload) {
            warn!(?err, "failed to emit contacts-synced event");
        }

        // Release the in-flight guard before returning.
        {
            let mut guard = self.in_flight.lock().expect("sync in-flight poisoned");
            *guard = false;
        }

        // Reset the interval clock so the next periodic tick is
        // measured from this manual / app-start pass, not from the
        // previous one.
        self.notify.notify_one();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn read_interval_minutes_defaults_to_sixty() {
        let (_tmp, db) = fresh_db();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        assert_eq!(scheduler.read_interval_minutes(), 60);
    }

    #[test]
    fn read_interval_minutes_honours_user_pref() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_SYNC_INTERVAL_MINUTES, "15")
            .unwrap();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        assert_eq!(scheduler.read_interval_minutes(), 15);
    }

    #[test]
    fn read_interval_minutes_clamps_zero_to_one() {
        // Defensive guard: a typo of `0` mustn't pin the worker
        // into a hot loop. We promote it to 1 so the pass still
        // runs once a minute at worst.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_SYNC_INTERVAL_MINUTES, "0")
            .unwrap();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        assert_eq!(scheduler.read_interval_minutes(), 1);
    }

    #[test]
    fn status_returns_empty_when_nothing_synced_yet() {
        let (_tmp, db) = fresh_db();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        let s = scheduler.status();
        assert!(s.last_synced_at.is_none());
        assert_eq!(s.interval_minutes, 60);
        assert!(!s.in_flight);
        // Pref defaults to false — fresh installs don't pay the
        // GAL-pull cost without opt-in.
        assert!(!s.include_read_only_on_sync);
    }

    #[test]
    fn read_include_read_only_on_sync_defaults_to_false() {
        let (_tmp, db) = fresh_db();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        assert!(!scheduler.read_include_read_only_on_sync());
    }

    #[test]
    fn read_include_read_only_on_sync_honours_true() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_INCLUDE_READ_ONLY_ON_SYNC, "true")
            .unwrap();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        assert!(scheduler.read_include_read_only_on_sync());
    }

    #[test]
    fn read_include_read_only_on_sync_other_values_are_false() {
        // Anything but the literal "true" reads false — defensive
        // against the case where a power user typed something
        // else into the pref via user_prefs editing.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_INCLUDE_READ_ONLY_ON_SYNC, "yes")
            .unwrap();
        let scheduler = ContactSyncScheduler {
            registry: Arc::new(AdapterRegistry::new()),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        };
        assert!(!scheduler.read_include_read_only_on_sync());
    }
}
