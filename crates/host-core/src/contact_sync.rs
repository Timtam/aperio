//! Contact-sync status (DESIGN.md §10.5) — the platform-agnostic half.
//!
//! The contacts sync IS the refresh of external data (Toni, 2026-11-09):
//! every warm pass of [`crate::cache::CacheRefresher`] reads the contact lists
//! of every external `ContactsFeature` adapter alongside the calendars and task
//! lists, on the one refresh interval (`sync.intervalMinutes`). "Sync contacts
//! now" is a contacts-only pass of the same refresher
//! ([`crate::cache::CacheRefresher::warm_contacts`]). A pass does **not**
//! persist contacts into our own SQLite — each adapter owns its own cache; the
//! frontend re-reads via `get_contacts` after the [`ContactSyncObserver`]
//! fires.
//!
//! This module keeps what is the contacts' own: the "last synced" time, the
//! in-flight flag the contacts view shows, the read-only-directories pref, and
//! the done signal. The refresher calls [`ContactSyncCore::begin_pass`] and
//! [`ContactSyncCore::finish_pass`] around each pass once a host installed the
//! core ([`crate::cache::CacheRefresher::set_contact_sync`]):
//!   - **Desktop** — an observer that emits the Tauri `contacts-synced` event.
//!   - **Mobile** (cal-ffi) — one that forwards the payload over a UniFFI
//!     callback.
//!
//! ## The read-only directories
//!
//! Read-only lists (the EWS GAL, Google Other Contacts / Directory, the Graph
//! Suggested People stream) carry thousands of rows. A periodic pass reads
//! them only when [`PREF_INCLUDE_READ_ONLY_ON_SYNC`] is on; the app-start
//! pass, a manual refresh and a change of an account read them as ever, and so
//! does opening one.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde::Serialize;
use tracing::warn;

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key for the timestamp of the most recent sync pass, persisted
/// as RFC 3339. Used by the contacts view's "Last synced at …" footer so the
/// value survives restarts.
pub const PREF_LAST_SYNCED_AT: &str = "contacts.lastSyncedAt";

/// `user_prefs` key for the "also pull read-only directories" toggle
/// (Settings → Kontakte). When `true`, every periodic pass reads the expensive
/// read-only sentinels (EWS GAL, Google Other Contacts / Workspace Directory,
/// MS Graph Suggested People). Defaults to `false` so a quiet account doesn't
/// pay the multi-minute scan cost on every interval without the user opting
/// in.
///
/// Stored as the literal strings `"true"` / `"false"` so the keychain debug
/// view stays readable. The manual sync path keeps an explicit override
/// parameter that beats this pref — used for a one-shot directory pull without
/// flipping the user-visible setting.
pub const PREF_INCLUDE_READ_ONLY_ON_SYNC: &str = "contacts.includeReadOnlyOnSync";

/// Read [`PREF_INCLUDE_READ_ONLY_ON_SYNC`]. Anything other than the literal
/// `"true"` (case-sensitive) reads as `false` — same lenient parse the sync
/// flag uses.
pub fn include_read_only_on_sync(db: &SharedConn) -> bool {
    UserPrefsRepo::new(db)
        .get(PREF_INCLUDE_READ_ONLY_ON_SYNC)
        .ok()
        .flatten()
        .as_deref()
        == Some("true")
}

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
    /// Another pass, which reads the contacts too, starts at once (a
    /// "Sync now" that waited behind this one, say): the sync is still
    /// running.
    pub in_flight: bool,
}

/// Status snapshot read by the contacts view's footer. The values
/// here come straight from `user_prefs` — no in-memory state past
/// what the persisted timestamps tell us.
#[derive(Debug, Clone, Serialize)]
pub struct ContactsSyncStatus {
    pub last_synced_at: Option<String>,
    pub in_flight: bool,
    /// Current value of [`PREF_INCLUDE_READ_ONLY_ON_SYNC`]. The
    /// Settings → Kontakte checkbox seeds itself from this so the
    /// UI doesn't need a separate `get_user_pref` round-trip.
    pub include_read_only_on_sync: bool,
}

/// The contacts' own share of a refresh pass: the "last synced" time, the
/// in-flight flag and the done signal. Construct with [`ContactSyncCore::new`]
/// and hand it to the refresher with
/// [`crate::cache::CacheRefresher::set_contact_sync`].
pub struct ContactSyncCore {
    db: SharedConn,
    /// Last sync time, kept in memory so a status read doesn't touch
    /// SQLite. Persisted to `user_prefs` in the same step so the value
    /// survives restarts.
    last_synced_at: Arc<Mutex<Option<DateTime<Utc>>>>,
    /// `true` while a refresh pass reads the contact lists.
    in_flight: Arc<Mutex<bool>>,
}

impl ContactSyncCore {
    /// Build the core, hydrating the in-memory `last_synced_at` from prefs so
    /// the first `status()` after restart reflects the persisted value without
    /// waiting for the first pass to complete.
    pub fn new(db: SharedConn) -> Arc<Self> {
        let initial_last = {
            let repo = UserPrefsRepo::new(&db);
            repo.get(PREF_LAST_SYNCED_AT)
                .ok()
                .flatten()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|d| d.with_timezone(&Utc))
        };
        Arc::new(Self {
            db,
            last_synced_at: Arc::new(Mutex::new(initial_last)),
            in_flight: Arc::new(Mutex::new(false)),
        })
    }

    /// Read the current value of [`PREF_INCLUDE_READ_ONLY_ON_SYNC`].
    pub fn read_include_read_only_on_sync(&self) -> bool {
        include_read_only_on_sync(&self.db)
    }

    /// Snapshot the current sync status.
    pub fn status(&self) -> ContactsSyncStatus {
        let last = self.last_synced_at.lock().expect("sync mutex poisoned");
        let in_flight = *self.in_flight.lock().expect("sync in-flight poisoned");
        ContactsSyncStatus {
            last_synced_at: last.map(|d| d.to_rfc3339()),
            in_flight,
            include_read_only_on_sync: self.read_include_read_only_on_sync(),
        }
    }

    /// A refresh pass starts reading the contact lists.
    pub fn begin_pass(&self) {
        *self.in_flight.lock().expect("sync in-flight poisoned") = true;
    }

    /// A refresh pass has read the contact lists: `succeeded` are the
    /// accounts all of whose lists came through, `failed` the ones where
    /// listing or a list threw. Persists "last synced" and tells `observer`;
    /// the in-flight flag stays set when `more_follows` (the next pass, which
    /// reads the contacts too, starts at once).
    pub fn finish_pass(
        &self,
        succeeded: Vec<String>,
        failed: Vec<String>,
        more_follows: bool,
        observer: &dyn ContactSyncObserver,
    ) {
        let now = Utc::now();
        *self.last_synced_at.lock().expect("sync mutex poisoned") = Some(now);
        if let Err(err) = UserPrefsRepo::new(&self.db).set(PREF_LAST_SYNCED_AT, &now.to_rfc3339()) {
            warn!(?err, "failed to persist contacts.lastSyncedAt");
        }
        *self.in_flight.lock().expect("sync in-flight poisoned") = more_follows;
        observer.contacts_synced(&ContactsSyncedPayload {
            last_synced_at: now.to_rfc3339(),
            succeeded_accounts: succeeded,
            failed_accounts: failed,
            in_flight: more_follows,
        });
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

    fn core_with(db: &DbHandle) -> Arc<ContactSyncCore> {
        ContactSyncCore::new(db.shared())
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<ContactsSyncedPayload>>);

    impl ContactSyncObserver for Recorder {
        fn contacts_synced(&self, payload: &ContactsSyncedPayload) {
            self.0.lock().unwrap().push(payload.clone());
        }
    }

    #[test]
    fn status_returns_empty_when_nothing_synced_yet() {
        let (_tmp, db) = fresh_db();
        let s = core_with(&db).status();
        assert!(s.last_synced_at.is_none());
        assert!(!s.in_flight);
        // Pref defaults to false — fresh installs don't pay the
        // GAL-pull cost without opt-in.
        assert!(!s.include_read_only_on_sync);
    }

    /// A pass shows as running from `begin_pass` to `finish_pass`, which
    /// persists "last synced" (a restart reads it back) and hands the
    /// accounts to the observer.
    #[test]
    fn a_pass_runs_from_begin_to_finish_and_is_remembered() {
        let (_tmp, db) = fresh_db();
        let core = core_with(&db);
        core.begin_pass();
        assert!(core.status().in_flight);
        let recorder = Recorder::default();
        // Another pass follows at once: still running.
        core.finish_pass(Vec::new(), Vec::new(), true, &recorder);
        assert!(core.status().in_flight);
        assert!(recorder.0.lock().unwrap()[0].in_flight);
        recorder.0.lock().unwrap().clear();
        core.finish_pass(vec!["a".into()], vec!["b".into()], false, &recorder);
        let status = core.status();
        assert!(!status.in_flight);
        let last = status.last_synced_at.expect("last synced is set");
        let seen = recorder.0.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].last_synced_at, last);
        assert_eq!(seen[0].succeeded_accounts, vec!["a".to_string()]);
        assert_eq!(seen[0].failed_accounts, vec!["b".to_string()]);
        assert!(!seen[0].in_flight);
        assert_eq!(core_with(&db).status().last_synced_at, Some(last));
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
