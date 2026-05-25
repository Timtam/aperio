//! User-prefs-backed SFTP host-key pin store (DESIGN.md §19.5).
//!
//! Stores accepted fingerprints under a per-host
//! `user_prefs.sync.adapter.sftp.knownHosts.<host:port>` key so
//! the TOFU pinning survives app restarts. The keys never leave
//! the device — they're not on the sync whitelist (see
//! `event_log::whitelist`), so even with cross-device sync
//! enabled each device pins independently.
//!
//! Iteration 9 split: the actual "verify a presented
//! fingerprint" decision moved into the SFTP plugin (which
//! receives the pinned fingerprint via init_config) — the host
//! no longer implements `HostKeyVerifier`. What stays here is
//! the pin store: peek (for `preview_sftp_host_key`'s
//! comparison + the build_adapter_from_prefs path's pinned
//! lookup), record (for `trust_sftp_host_key`'s user-confirmed
//! acceptance), and forget (for the "Vergessen" button).

use tracing::warn;

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key prefix. The full key is
/// `<prefix><host>:<port>` — colons in the host_port string are
/// fine here; user_prefs stores opaque strings.
const PREFIX: &str = "sync.adapter.sftp.knownHosts.";

/// Concrete pin store backed by the SQLite-backed `user_prefs`
/// table. Wraps a `SharedConn` clone.
#[derive(Debug)]
pub struct UserPrefsHostKeyVerifier {
    db: SharedConn,
}

impl UserPrefsHostKeyVerifier {
    pub fn new(db: SharedConn) -> Self {
        Self { db }
    }

    fn key_for(host_port: &str) -> String {
        format!("{PREFIX}{host_port}")
    }

    /// Look up the pinned fingerprint for `host_port`, or
    /// `None` if nothing is pinned yet. Used by
    /// `preview_sftp_host_key` (to classify a freshly-probed
    /// fingerprint as New / Unchanged / Changed) and by
    /// `pinned_sftp_fingerprint` (to thread the pin into the
    /// plugin's init_config).
    pub fn peek(&self, host_port: &str) -> Option<String> {
        let repo = UserPrefsRepo::new(&self.db);
        match repo.get(&Self::key_for(host_port)) {
            Ok(s) => s,
            Err(err) => {
                // A transient read failure shouldn't trap the
                // user. Returning None lets the preview path
                // fall back to the "first use" dialog, which is
                // recoverable.
                warn!(
                    ?err,
                    host_port = %host_port,
                    "couldn't read SFTP known-host entry for peek; \
                     treating as none",
                );
                None
            }
        }
    }

    /// Persist the fingerprint as the user-confirmed pin for
    /// `host_port`. Called only from `trust_sftp_host_key` —
    /// pinning is always an explicit user gesture (§19.5).
    pub fn record(&self, host_port: &str, fingerprint: &str) {
        let repo = UserPrefsRepo::new(&self.db);
        if let Err(err) = repo.set(&Self::key_for(host_port), fingerprint) {
            warn!(
                ?err,
                host_port = %host_port,
                "couldn't persist SFTP host-key fingerprint",
            );
        }
    }

    /// Drop the pinned fingerprint for `host_port`. Driven by
    /// the "Vergessen" button in the SyncPanel.
    pub fn forget(&self, host_port: &str) {
        let repo = UserPrefsRepo::new(&self.db);
        if let Err(err) = repo.delete(&Self::key_for(host_port)) {
            warn!(
                ?err,
                host_port = %host_port,
                "couldn't drop SFTP host-key fingerprint",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(&dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }

    #[test]
    fn peek_returns_none_for_unknown_host() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        assert_eq!(v.peek("nas:22"), None);
    }

    #[test]
    fn record_persists_for_subsequent_peek() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        assert_eq!(v.peek("nas:22"), Some("SHA256:abc".into()));
    }

    #[test]
    fn record_overwrites_previous_pin() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        v.record("nas:22", "SHA256:xyz");
        assert_eq!(v.peek("nas:22"), Some("SHA256:xyz".into()));
    }

    #[test]
    fn forget_drops_persisted_pin() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        v.forget("nas:22");
        assert_eq!(v.peek("nas:22"), None);
    }

    #[test]
    fn forget_unknown_host_is_noop() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        // Doesn't panic when nothing is pinned.
        v.forget("nas:22");
    }

    #[test]
    fn peek_doesnt_mutate_state() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        let _ = v.peek("nas:22");
        let _ = v.peek("nas:22");
        // Still the same entry after multiple peeks.
        assert_eq!(v.peek("nas:22"), Some("SHA256:abc".into()));
    }

    #[test]
    fn separate_hosts_dont_collide() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas-a:22", "SHA256:a");
        v.record("nas-b:22", "SHA256:b");
        assert_eq!(v.peek("nas-a:22"), Some("SHA256:a".into()));
        assert_eq!(v.peek("nas-b:22"), Some("SHA256:b".into()));
    }
}
