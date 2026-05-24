//! User-prefs-backed [`HostKeyVerifier`] for the SFTP sync
//! adapter (DESIGN.md §19.5).
//!
//! Stores accepted fingerprints under a per-host
//! `user_prefs.sync.adapter.sftp.knownHosts.<host:port>` key so
//! the TOFU pinning survives app restarts. The keys never leave
//! the device — they're not on the sync whitelist (see
//! `event_log::whitelist`), so even with cross-device sync
//! enabled each device pins independently.
//!
//! ## Threading concerns
//!
//! `HostKeyVerifier::verify` + `record` run on the russh
//! handshake task, which is async. Our [`UserPrefsRepo`] holds a
//! `std::sync::Mutex` around the SQLite connection; the lock
//! window is microseconds, so a blocking `lock()` from the async
//! context is fine (no `.await` happens while it's held).

use sync_adapter_sftp::{HostKeyDecision, HostKeyVerifier};
use tracing::warn;

use crate::db::SharedConn;
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key prefix. The full key is
/// `<prefix><host>:<port>` — colons in the host_port string are
/// fine here; user_prefs stores opaque strings.
const PREFIX: &str = "sync.adapter.sftp.knownHosts.";

/// Concrete verifier backed by the SQLite-backed `user_prefs`
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
}

impl HostKeyVerifier for UserPrefsHostKeyVerifier {
    fn verify(&self, host_port: &str, fingerprint: &str) -> HostKeyDecision {
        let repo = UserPrefsRepo::new(&self.db);
        let stored = match repo.get(&Self::key_for(host_port)) {
            Ok(s) => s,
            Err(err) => {
                // Read failure is unusual — log and treat as
                // "unknown host" so the user gets a fresh TOFU
                // prompt rather than a permanent blocker.
                warn!(
                    ?err,
                    host_port = %host_port,
                    "couldn't read SFTP known-host entry; treating as new",
                );
                None
            }
        };
        match stored {
            None => HostKeyDecision::AcceptAndRemember,
            Some(s) if s == fingerprint => HostKeyDecision::Accept,
            Some(s) => HostKeyDecision::Mismatch {
                stored: s,
                presented: fingerprint.to_string(),
            },
        }
    }

    fn peek(&self, host_port: &str) -> Option<String> {
        let repo = UserPrefsRepo::new(&self.db);
        match repo.get(&Self::key_for(host_port)) {
            Ok(s) => s,
            Err(err) => {
                // Same treatment as `verify`: a transient read
                // failure shouldn't trap the user. Returning None
                // lets the preview path fall back to the "first
                // use" dialog, which is recoverable.
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

    fn record(&self, host_port: &str, fingerprint: &str) {
        let repo = UserPrefsRepo::new(&self.db);
        if let Err(err) = repo.set(&Self::key_for(host_port), fingerprint) {
            warn!(
                ?err,
                host_port = %host_port,
                "couldn't persist SFTP host-key fingerprint",
            );
        }
    }

    fn forget(&self, host_port: &str) {
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
    fn first_use_returns_accept_and_remember() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        assert_eq!(
            v.verify("nas:22", "SHA256:abc"),
            HostKeyDecision::AcceptAndRemember,
        );
    }

    #[test]
    fn record_persists_for_subsequent_verify() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        assert_eq!(
            v.verify("nas:22", "SHA256:abc"),
            HostKeyDecision::Accept,
        );
    }

    #[test]
    fn mismatch_returns_stored_and_presented() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        assert_eq!(
            v.verify("nas:22", "SHA256:xyz"),
            HostKeyDecision::Mismatch {
                stored: "SHA256:abc".into(),
                presented: "SHA256:xyz".into(),
            },
        );
    }

    #[test]
    fn peek_returns_none_for_unknown_host() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        assert_eq!(v.peek("nas:22"), None);
    }

    #[test]
    fn peek_returns_stored_fingerprint() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        assert_eq!(v.peek("nas:22"), Some("SHA256:abc".into()));
    }

    #[test]
    fn forget_drops_persisted_pin() {
        let (_tmp, db) = fresh_db();
        let v = UserPrefsHostKeyVerifier::new(db.shared());
        v.record("nas:22", "SHA256:abc");
        v.forget("nas:22");
        assert_eq!(v.peek("nas:22"), None);
        // The next verify should treat this as first-use.
        assert_eq!(
            v.verify("nas:22", "SHA256:xyz"),
            HostKeyDecision::AcceptAndRemember,
        );
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
        assert_eq!(
            v.verify("nas-a:22", "SHA256:a"),
            HostKeyDecision::Accept,
        );
        assert_eq!(
            v.verify("nas-b:22", "SHA256:b"),
            HostKeyDecision::Accept,
        );
        // Cross-check returns Mismatch since the other host's
        // fingerprint doesn't match.
        assert!(matches!(
            v.verify("nas-a:22", "SHA256:b"),
            HostKeyDecision::Mismatch { .. },
        ));
    }
}
