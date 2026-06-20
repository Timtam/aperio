//! Contact-sync core (DESIGN.md §10.5) — the platform-agnostic half.
//!
//! Walks every external `ContactsFeature` adapter and warms its listing +
//! per-list caches by calling `list_contact_lists` followed by `get_contacts`
//! on every (writable) list. A pass does **not** persist data into our own
//! SQLite — each adapter owns its own cache (5-minute TTL); the frontend
//! re-reads via `get_contacts` after the [`ContactSyncObserver`] fires.
//!
//! This is the shared core: the pure sync work + status + the `user_prefs`
//! readers. The *driving* lives in the platform layer, mirroring the
//! [`crate::sync`] `SyncOrchestrator`(core) / desktop `SyncScheduler`(loop)
//! split and the [`crate::cache`] `CacheObserver` seam:
//!   - **Desktop** wraps this in a tokio worker loop (app-start + periodic
//!     ticks) and an observer that emits the Tauri `contacts-synced` event.
//!   - **Mobile** (cal-ffi) drives [`ContactSyncCore::run_sync`] from JS
//!     foreground triggers and forwards the payload over a UniFFI callback.
//!
//! ## What gets pulled per pass
//!
//! Only **writable** lists are pulled by default. Read-only sentinels (the EWS
//! GAL, Google Other Contacts / Directory, the Graph Suggested People stream)
//! carry thousands of rows and are paid for only on explicit user action —
//! unless `include_read_only` is set (the "force-pull everything" path).
//!
//! ## In-flight dedupe
//!
//! A `Mutex<bool>` guards against overlapping passes — ten rapid triggers run
//! exactly one walk; subsequent calls return `false`.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::Notify;
use tracing::{debug, warn};

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
/// toggle (Settings → Kontakte). When `true`, every sync pass walks
/// the expensive read-only sentinels (EWS GAL, Google Other
/// Contacts / Workspace Directory, MS Graph Suggested People).
/// Defaults to `false` so a quiet account doesn't pay the
/// multi-minute scan cost without the user opting in.
///
/// Stored as the literal strings `"true"` / `"false"` so the keychain
/// debug view stays readable. The manual sync path keeps an explicit
/// override parameter that beats this pref — used for a one-shot
/// directory pull without flipping the user-visible setting.
pub const PREF_INCLUDE_READ_ONLY_ON_SYNC: &str = "contacts.includeReadOnlyOnSync";

/// Default interval if `PREF_SYNC_INTERVAL_MINUTES` is unset or
/// nonsense. 60 minutes per the design doc.
pub const DEFAULT_SYNC_INTERVAL_MINUTES: u32 = 60;

/// Host-supplied sink for the "a contact-sync pass finished" broadcast. The
/// core calls this at the end of every pass instead of touching any UI layer:
/// the desktop impl forwards to the Tauri `contacts-synced` event; the mobile
/// UniFFI host JSONs the payload across the FFI bridge (exactly like
/// [`crate::cache`]'s `CacheObserver`). `ContactsSyncedPayload` is `Serialize`,
/// so the mobile bridge can serialise it directly.
pub trait ContactSyncObserver: Send + Sync {
    fn contacts_synced(&self, payload: &ContactsSyncedPayload);
}

/// Frontend payload for the `contacts-synced` broadcast the core emits after a
/// pass. The frontend uses `last_synced_at` to update the "Last synced at …"
/// footer and to invalidate its SWR cache so a subsequent refetch picks up the
/// freshly-warmed adapter cache.
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
    /// UI doesn't need a separate `get_user_pref` round-trip.
    pub include_read_only_on_sync: bool,
}

/// The platform-agnostic contact-sync core: the pure sync work + status + the
/// `user_prefs` readers. Construct with [`ContactSyncCore::new`]; drive
/// [`ContactSyncCore::run_sync`] from a platform worker loop (desktop) or
/// foreground triggers (mobile).
pub struct ContactSyncCore {
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
    /// Lets a manual trigger wake the desktop periodic loop out of its
    /// sleep so the next interval starts counting from the manual kick
    /// rather than the previous tick. `notify_one()` fires inside
    /// `run_sync`; the desktop loop awaits `notify_handle().notified()`.
    notify: Arc<Notify>,
}

impl ContactSyncCore {
    /// Build the core, hydrating the in-memory `last_synced_at` from prefs so
    /// the first `status()` after restart reflects the persisted value without
    /// waiting for the first pass to complete.
    pub fn new(registry: Arc<AdapterRegistry>, db: SharedConn) -> Arc<Self> {
        let initial_last = {
            let repo = UserPrefsRepo::new(&db);
            repo.get(PREF_LAST_SYNCED_AT)
                .ok()
                .flatten()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc))
        };
        Arc::new(Self {
            registry,
            db,
            last_synced_at: Arc::new(Mutex::new(initial_last)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        })
    }

    /// The wake handle for the desktop periodic loop: its `select!` races
    /// `notify_handle().notified()` against the interval sleep so a manual
    /// trigger resets the interval clock. The `notify_one()` reset lives
    /// inside [`ContactSyncCore::run_sync`].
    pub fn notify_handle(&self) -> Arc<Notify> {
        Arc::clone(&self.notify)
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
    /// reads as `false` — same lenient parse the sync flag uses.
    pub fn read_include_read_only_on_sync(&self) -> bool {
        let repo = UserPrefsRepo::new(&self.db);
        repo.get(PREF_INCLUDE_READ_ONLY_ON_SYNC)
            .ok()
            .flatten()
            .as_deref()
            == Some("true")
    }

    /// Snapshot the current sync status.
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
    /// expensive read-only sentinel lists (GAL etc.) — `false` on the
    /// auto-triggered paths, `true` only on a manual "force everything".
    /// `observer` receives the [`ContactsSyncedPayload`] when the pass
    /// completes.
    ///
    /// Returns `true` when this call actually ran the pass, `false`
    /// when another pass was already in flight and this invocation
    /// was deduped.
    pub async fn run_sync(
        &self,
        observer: &dyn ContactSyncObserver,
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
        observer.contacts_synced(&payload);

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

    /// Empty registry for the contact-sync core tests. None of these
    /// tests exercise the per-account adapter routing, so an empty
    /// plugin manager is sufficient — the registry just needs to be
    /// constructible.
    fn empty_registry() -> Arc<AdapterRegistry> {
        let mgr = Arc::new(plugin_core::PluginManager::new("0.1.0"));
        let secrets = Arc::new(sync_engine::test_support::FakeSecrets::default());
        Arc::new(AdapterRegistry::new(mgr, secrets))
    }

    fn core_with(db: &DbHandle) -> ContactSyncCore {
        ContactSyncCore {
            registry: empty_registry(),
            db: db.shared(),
            last_synced_at: Arc::new(Mutex::new(None)),
            in_flight: Arc::new(Mutex::new(false)),
            notify: Arc::new(Notify::new()),
        }
    }

    #[test]
    fn read_interval_minutes_defaults_to_sixty() {
        let (_tmp, db) = fresh_db();
        assert_eq!(core_with(&db).read_interval_minutes(), 60);
    }

    #[test]
    fn read_interval_minutes_honours_user_pref() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_SYNC_INTERVAL_MINUTES, "15")
            .unwrap();
        assert_eq!(core_with(&db).read_interval_minutes(), 15);
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
        assert_eq!(core_with(&db).read_interval_minutes(), 1);
    }

    #[test]
    fn status_returns_empty_when_nothing_synced_yet() {
        let (_tmp, db) = fresh_db();
        let s = core_with(&db).status();
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
        assert!(!core_with(&db).read_include_read_only_on_sync());
    }

    #[test]
    fn read_include_read_only_on_sync_honours_true() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        UserPrefsRepo::new(&shared)
            .set(PREF_INCLUDE_READ_ONLY_ON_SYNC, "true")
            .unwrap();
        assert!(core_with(&db).read_include_read_only_on_sync());
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
        assert!(!core_with(&db).read_include_read_only_on_sync());
    }
}
