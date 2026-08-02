//! SFTP `SyncAdapter` implementation (DESIGN.md §19.6).
//!
//! Pure-Rust SSH client (`russh` + `russh-sftp`) so the adapter
//! compiles for Mobile targets too — no libssh2 system dependency.
//!
//! Maps the `sync_core::SyncAdapter` trait onto SFTP file
//! operations:
//!
//! | Trait call           | SFTP operation                              |
//! |----------------------|---------------------------------------------|
//! | `test_connection`    | Open SSH + SFTP session, list base dir.     |
//! | `fetch_meta`         | `open(<base>/meta.json)` + read-to-end       |
//! | `push_meta`          | `create + write` over `<base>/meta.json`     |
//! | `fetch_new_logs`     | `read_dir(<base>/log/)` + per-file read     |
//! | `push_log`           | write `<base>/log/<filename>`                |
//! | `fetch/push_snap`    | GET / PUT `<base>/snapshot.json`             |
//! | `delete_log`         | `remove_file`                               |
//! | sound asset CRUD     | `<base>/assets/sounds/<hash>.<ext>`          |
//!
//! ## Connection lifecycle
//!
//! One live SSH + SFTP session is cached on the adapter (shared
//! across clones) and reused by every trait call. v1 opened a
//! fresh session per call — 2 full handshakes for a no-op round,
//! 5 for a push round, each ≈100-300 ms of TCP + key exchange +
//! auth + subsystem setup, plus the key-file read + passphrase
//! KDF for key auth — and profiling showed that overhead
//! dominating a round's wall time. Cached sessions can die idle
//! between rounds (NAT boxes / home routers reap quiet TCP
//! flows), so an operation error on a reused session drops it
//! and transparently retries once on a fresh connection; see
//! [`SftpSyncAdapter::with_session`]. The parsed private key is
//! cached separately so reconnects skip the deliberately slow
//! bcrypt-pbkdf on encrypted keys.
//!
//! ## Host-key verification (TOFU)
//!
//! `ClientHandler::check_server_key` consults a
//! [`HostKeyVerifier`] supplied by the caller. The convention is
//! trust-on-first-use:
//!
//! - **Unknown host**: first connect ever to this host:port —
//!   accept + remember the fingerprint.
//! - **Known and matching**: accept silently.
//! - **Known and mismatching**: reject the handshake.
//!   `connect()` translates the russh disconnect into a
//!   [`SyncError::Auth`] with a "host key changed" message; the
//!   command layer surfaces it to the user via a distinct error
//!   code so the Settings panel can render the §19.5 "verify
//!   the server identity out-of-band" warning instead of a
//!   generic auth failure.
//!
//! The [`InMemoryHostKeyVerifier`] is the test fixture; the
//! production [`UserPrefsHostKeyVerifier`] lives in `src-tauri`
//! against the `user_prefs.sync.adapter.sftp.knownHosts.*` keys.
//!
//! ## Auth
//!
//! Two methods supported:
//!
//! - [`SftpAuth::Password { password }`]
//! - [`SftpAuth::PrivateKey { path, passphrase }`] — PEM or
//!   OpenSSH-format private key, with optional passphrase.
//!
//! ## What this crate does NOT do
//!
//! - **Resume / partial uploads.** Each write is one round-trip;
//!   meta.json + snapshot.json use atomic write-temp + rename so
//!   a crash mid-write can't leave a corrupt control file. Log
//!   files are write-once with timestamp + device id in the
//!   name, so a partial write is retried naturally by the
//!   scheduler picking up the same pending file.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::{self, StreamExt};
use russh::client::{self, Handle};
use russh::keys::ssh_key::{Algorithm, HashAlg, PublicKey};
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use serde::{Deserialize, Serialize};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter, SyncError, SyncResult,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::debug;

// ─────────────────────────────────────────────────────────────────
// Auth + host-key verifier types
// ─────────────────────────────────────────────────────────────────

/// Authentication method the adapter uses against the SSH server.
///
/// `Password` and `PrivateKey` cover the two cases v1 supports.
/// Future variants (agent forwarding, hardware key) can extend
/// this enum without changing the trait surface.
#[derive(Debug, Clone)]
pub enum SftpAuth {
    Password {
        password: String,
    },
    PrivateKey {
        /// Absolute path to a PEM or OpenSSH-format private key
        /// file. We don't keep the key material itself in memory
        /// until the connect path actually reads it — protects
        /// against accidental serialisation.
        path: PathBuf,
        /// Optional passphrase for an encrypted key. `None` for
        /// unencrypted keys; an empty string is treated as `None`.
        passphrase: Option<String>,
    },
}

/// Decision returned by a [`HostKeyVerifier`] for a server-
/// presented public key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostKeyDecision {
    /// First contact with this host — record + accept.
    AcceptAndRemember,
    /// Known host with matching fingerprint — accept silently.
    Accept,
    /// Known host but the fingerprint changed since last connect.
    /// Includes both the stored + presented forms so the
    /// surfaced error can show them to the user.
    Mismatch { stored: String, presented: String },
}

/// Snapshot the UI uses to decide between the "first use" and
/// "fingerprint changed" trust dialogs. Built by
/// [`SftpSyncAdapter::preview_host_key`] — see that method for the
/// usage flow.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostKeyPreview {
    /// `"host:port"` form, e.g. `"nas.example.com:22"`. Echoes
    /// back to the UI so the dialog can show it verbatim without
    /// re-deriving from the config.
    pub host_port: String,
    /// The SHA256 fingerprint the server presented right now.
    pub fingerprint: String,
    /// What this fingerprint means relative to whatever the
    /// verifier already has stored.
    pub status: HostKeyPreviewStatus,
}

/// Result of comparing the freshly-observed fingerprint against
/// whatever the verifier has pinned for this `host:port`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HostKeyPreviewStatus {
    /// No entry on file — the user is about to TOFU-pin this
    /// server for the first time.
    New,
    /// Matches the stored fingerprint exactly; the UI can skip the
    /// confirmation dialog and proceed straight to configure.
    Unchanged,
    /// A different fingerprint is on file. The UI MUST show the
    /// stored + presented values side-by-side and force an
    /// explicit user gesture before pinning the new key.
    Changed {
        /// The fingerprint that was previously pinned.
        stored: String,
    },
}

/// Decides whether to accept a server's host key.
///
/// Implementors are called from inside the SSH handshake, so
/// they MUST be cheap + non-blocking. The default
/// [`InMemoryHostKeyVerifier`] is fine for tests; the production
/// `UserPrefsHostKeyVerifier` reads + writes `user_prefs` from
/// `src-tauri`.
///
/// `host_port` is the `"host:port"` string used as the lookup
/// key; the verifier doesn't need to parse it. `fingerprint` is
/// the standard SHA-256 fingerprint of the server's public key
/// (`SHA256:<base64>` form per ssh-keygen).
pub trait HostKeyVerifier: Send + Sync + std::fmt::Debug {
    /// Look up the stored fingerprint for `host_port` and decide
    /// what to do with the presented one. Pure read; never
    /// mutates the verifier's persisted state.
    fn verify(&self, host_port: &str, fingerprint: &str) -> HostKeyDecision;
    /// Borrow the stored fingerprint for `host_port` without
    /// changing it. Used by the preview path so the UI can show
    /// the user both the stored and the presented key side-by-
    /// side on a mismatch — and so it can tell "first use" apart
    /// from "key changed".
    fn peek(&self, host_port: &str) -> Option<String>;
    /// Commit a TOFU acceptance — called by the command layer
    /// AFTER the user has confirmed the fingerprint in the UI,
    /// or by the adapter itself after a silent-TOFU connect for
    /// callers that don't need the confirmation step.
    fn record(&self, host_port: &str, fingerprint: &str);
    /// Drop the stored fingerprint for `host_port`. No-op when
    /// nothing is pinned. Used by the SyncPanel's "Pin
    /// vergessen" gesture — a user who knows their server's key
    /// rotated can clear the old pin proactively instead of
    /// waiting for the next connect to trip the mismatch
    /// detector. The next connect to this host_port will go
    /// through the first-use trust dialog again.
    fn forget(&self, host_port: &str);
}

/// In-memory implementation for tests. Stores known hosts in a
/// `Mutex<HashMap>`; not persisted anywhere.
#[derive(Debug, Default)]
pub struct InMemoryHostKeyVerifier {
    known: Mutex<std::collections::HashMap<String, String>>,
}

impl InMemoryHostKeyVerifier {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pre-seed a known fingerprint — handy for tests that want
    /// to assert the mismatch path.
    pub fn with_known(host_port: &str, fingerprint: &str) -> Self {
        let mut map = std::collections::HashMap::new();
        map.insert(host_port.to_string(), fingerprint.to_string());
        Self {
            known: Mutex::new(map),
        }
    }
}

impl HostKeyVerifier for InMemoryHostKeyVerifier {
    fn verify(&self, host_port: &str, fingerprint: &str) -> HostKeyDecision {
        let known = self.known.lock().expect("known-hosts mutex poison");
        match known.get(host_port) {
            None => HostKeyDecision::AcceptAndRemember,
            Some(stored) if stored == fingerprint => HostKeyDecision::Accept,
            Some(stored) => HostKeyDecision::Mismatch {
                stored: stored.clone(),
                presented: fingerprint.to_string(),
            },
        }
    }

    fn peek(&self, host_port: &str) -> Option<String> {
        self.known
            .lock()
            .expect("known-hosts mutex poison")
            .get(host_port)
            .cloned()
    }

    fn record(&self, host_port: &str, fingerprint: &str) {
        self.known
            .lock()
            .expect("known-hosts mutex poison")
            .insert(host_port.to_string(), fingerprint.to_string());
    }

    fn forget(&self, host_port: &str) {
        self.known
            .lock()
            .expect("known-hosts mutex poison")
            .remove(host_port);
    }
}

/// SFTP adapter configuration.
///
/// `host` is bare ("nas.example.com"); the adapter prepends
/// `port` at connect time. `base_path` is an absolute path on the
/// remote host (e.g. `/home/alice/aperio`) — Aperio's log/, meta,
/// snapshot, and assets/ live directly under it.
#[derive(Clone)]
pub struct SftpSyncAdapter {
    host: String,
    port: u16,
    user: String,
    auth: SftpAuth,
    base_path: PathBuf,
    /// Verifier consulted on every handshake. The default is an
    /// in-memory store; production wires a `UserPrefsHostKeyVerifier`
    /// so the TOFU pinning survives restarts.
    host_key_verifier: Arc<dyn HostKeyVerifier>,
    /// The live SSH + SFTP session reused across trait calls,
    /// shared across clones. `None` until the first operation
    /// connects. An async mutex because the slot stays locked for
    /// the whole operation — adapter calls are serialized on
    /// purpose; per-file parallelism happens INSIDE one operation
    /// over the one session (SFTP multiplexes request ids).
    session: Arc<tokio::sync::Mutex<Option<Arc<SessionPair>>>>,
    /// Parsed private key, cached after the first successful load.
    /// Reading + decrypting an OpenSSH key (bcrypt-pbkdf) is
    /// deliberately slow; paying it once per adapter lifetime
    /// instead of once per connect keeps reconnects cheap.
    /// Invalidated when publickey auth fails, so a key file
    /// replaced on disk is re-read on the next attempt instead of
    /// wedging every reconnect until app restart.
    key_cache: Arc<Mutex<Option<Arc<russh::keys::PrivateKey>>>>,
    /// Directories already ensured this session (webdav's MKCOL
    /// `ensured` cache, ported). SFTP directories persist
    /// server-side, so re-walking `mkdir_p` before every push just
    /// burns one round trip per path component. Shared across
    /// clones so the cache survives `.clone()`; cleared by
    /// [`Self::connect`] so it never outlives the session whose
    /// walks populated it.
    ensured: Arc<Mutex<HashSet<String>>>,
}

impl std::fmt::Debug for SftpSyncAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Manual impl: the cached session's russh handles aren't
        // `Debug`, and skipping `auth` keeps credentials out of
        // debug logs.
        f.debug_struct("SftpSyncAdapter")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("base_path", &self.base_path)
            .finish_non_exhaustive()
    }
}

/// A live SSH connection + SFTP subsystem session. The two travel
/// together because dropping the SSH handle tears down the
/// transport out from under the `SftpSession`.
struct SessionPair {
    /// Never used after connect; exists so its `Drop` closes the
    /// connection when the pair is discarded.
    _handle: Handle<ClientHandler>,
    sftp: SftpSession,
}

impl SftpSyncAdapter {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        auth: SftpAuth,
        base_path: impl Into<PathBuf>,
        host_key_verifier: Arc<dyn HostKeyVerifier>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            auth,
            base_path: base_path.into(),
            host_key_verifier,
            session: Arc::new(tokio::sync::Mutex::new(None)),
            key_cache: Arc::new(Mutex::new(None)),
            ensured: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Convenience constructor for password auth with the
    /// in-memory verifier — used by tests and downstream code
    /// that doesn't need persistent host-key pinning.
    pub fn new_password(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        password: impl Into<String>,
        base_path: impl Into<PathBuf>,
    ) -> Self {
        Self::new(
            host,
            port,
            user,
            SftpAuth::Password {
                password: password.into(),
            },
            base_path,
            Arc::new(InMemoryHostKeyVerifier::new()),
        )
    }

    /// Borrow the configured base path. Used by Settings for the
    /// "current adapter: sftp://…" display.
    pub fn base_path(&self) -> &std::path::Path {
        &self.base_path
    }

    /// Open a fresh SSH connection just long enough to capture
    /// the server's SHA-256 host-key fingerprint, then drop the
    /// connection without authenticating. Used by the UI to
    /// preview the fingerprint before committing a TOFU pin or
    /// accepting a key change.
    ///
    /// Doesn't consult or mutate the configured `HostKeyVerifier`
    /// — the probe is purely informational.
    pub async fn probe_host_key_fingerprint(&self) -> SyncResult<String> {
        let captured = Arc::new(Mutex::new(None::<String>));
        let handler = ProbeHandler {
            captured: Arc::clone(&captured),
        };
        let config = Arc::new(client::Config::default());
        // Connect. The handler captures the fingerprint then
        // returns false, so russh aborts the handshake — we
        // expect Err here. The captured side channel carries
        // the result.
        let _ = client::connect(config, (self.host.as_str(), self.port), handler).await;
        let fp = captured
            .lock()
            .expect("probe capture poison")
            .take()
            .ok_or_else(|| {
                SyncError::network("didn't observe a host key before connection closed")
            })?;
        Ok(fp)
    }

    /// Compute a [`HostKeyPreview`] for the configured server:
    /// probe the fingerprint + compare against what the
    /// `HostKeyVerifier` currently has stored. The UI calls this
    /// to decide between the "first use" and "mismatch" trust
    /// dialogs.
    pub async fn preview_host_key(&self) -> SyncResult<HostKeyPreview> {
        let presented = self.probe_host_key_fingerprint().await?;
        let host_port = format!("{}:{}", self.host, self.port);
        let stored = self.host_key_verifier.peek(&host_port);
        let status = match stored.as_deref() {
            None => HostKeyPreviewStatus::New,
            Some(s) if s == presented => HostKeyPreviewStatus::Unchanged,
            Some(s) => HostKeyPreviewStatus::Changed {
                stored: s.to_string(),
            },
        };
        Ok(HostKeyPreview {
            host_port,
            fingerprint: presented,
            status,
        })
    }

    /// Borrow the host-key verifier so the command layer can
    /// call `record()` after the user confirms a fingerprint in
    /// the trust dialog. The orchestrator never calls record on
    /// its own; pinning is an explicit user gesture.
    pub fn host_key_verifier(&self) -> &Arc<dyn HostKeyVerifier> {
        &self.host_key_verifier
    }

    /// Concatenate the base path with a relative segment using
    /// forward slashes — SFTP paths are always POSIX-style even
    /// when the local OS is Windows.
    fn remote_path(&self, relative: &str) -> String {
        let base = self.base_path.to_string_lossy().replace('\\', "/");
        let trimmed_base = base.trim_end_matches('/');
        let trimmed_rel = relative.trim_start_matches('/');
        if trimmed_rel.is_empty() {
            trimmed_base.to_string()
        } else {
            format!("{trimmed_base}/{trimmed_rel}")
        }
    }

    /// Open a fresh SSH + SFTP session. Callers normally go through
    /// [`Self::with_session`], which caches the returned pair;
    /// dropping the last `Arc` closes the connection.
    async fn connect(&self) -> SyncResult<Arc<SessionPair>> {
        // Needing a NEW connection means the server state is
        // suspect (reset? dataset wiped? a previous mkdir walk may
        // have run on a dead session) — drop the ensured-directory
        // cache so callers re-create what's missing instead of
        // assuming it's intact. This is what keeps the cache to
        // its documented per-session lifetime: with_session's
        // reconnect-once retry re-runs `ensure_dir` against the
        // fresh session with an empty cache. Costs one extra
        // mkdir walk per reconnect — what v1 paid on every push.
        self.ensured.lock().expect("ensured mutex poison").clear();
        // The handler captures a shared side-channel slot for
        // verdicts. `check_server_key` writes the verifier's
        // [`HostKeyDecision`] (plus the observed fingerprint)
        // into the slot, then returns `false` for `Mismatch` so
        // russh aborts the handshake cleanly; for the two accept
        // variants it returns `true`. We read the slot after the
        // handshake to decide whether to commit a TOFU record.
        let outcome = Arc::new(Mutex::new(None::<HandshakeOutcome>));
        let host_port = format!("{}:{}", self.host, self.port);
        let handler = ClientHandler {
            verifier: Arc::clone(&self.host_key_verifier),
            host_port: host_port.clone(),
            outcome: Arc::clone(&outcome),
        };
        let config = Arc::new(client::Config::default());
        let connect_result =
            client::connect(config, (self.host.as_str(), self.port), handler).await;

        // Inspect the side channel BEFORE the connect error is
        // surfaced — a mismatch verdict beats a generic russh
        // disconnect message.
        let recorded_outcome = outcome.lock().expect("handshake outcome poison").take();
        let mut handle = match connect_result {
            Ok(h) => h,
            Err(err) => {
                if let Some(HandshakeOutcome {
                    decision: HostKeyDecision::Mismatch { stored, presented },
                    ..
                }) = recorded_outcome
                {
                    return Err(SyncError::auth(format!(
                        "host key mismatch — stored {stored}, server \
                         presented {presented}; verify the server \
                         out-of-band before re-connecting",
                    )));
                }
                return Err(SyncError::network(format!("ssh connect: {err}",)));
            }
        };

        // Authenticate. The auth call is allowed to fail with
        // server-side rejection (wrong password / key not in
        // authorized_keys); both map to `SyncError::Auth`.
        match &self.auth {
            SftpAuth::Password { password } => {
                let authed = handle
                    .authenticate_password(self.user.as_str(), password)
                    .await
                    .map_err(|err| SyncError::auth(format!("ssh auth: {err}")))?;
                if !authed.success() {
                    return Err(SyncError::auth(
                        "SSH password authentication rejected by server",
                    ));
                }
            }
            SftpAuth::PrivateKey { path, passphrase } => {
                let key = self.cached_private_key(path, passphrase.as_deref())?;
                // Pick the hash algorithm. RSA needs an explicit
                // SHA-256/SHA-512 (`Some(...)`); modern Ed25519 /
                // ECDSA keys return `None` which signals "use
                // whatever the algorithm naturally hashes with".
                //
                // russh exposes this via a private trait
                // (`helpers::algorithm::AlgorithmExt`); we inline
                // the four-line match here rather than fight the
                // visibility.
                let alg = hash_alg_for(&key.algorithm());
                let auth_key = russh::keys::PrivateKeyWithHashAlg::new(key, alg);
                let authed = handle
                    .authenticate_publickey(self.user.as_str(), auth_key)
                    .await
                    .map_err(|err| {
                        // A failed attempt is exactly when the
                        // cached key may be stale (rotated on disk
                        // since we parsed it) — drop it so the next
                        // connect re-reads the file instead of
                        // presenting the old key until app restart.
                        // The happy path still pays the KDF once.
                        *self.key_cache.lock().expect("key cache poison") = None;
                        SyncError::auth(format!("ssh key auth: {err}"))
                    })?;
                if !authed.success() {
                    // Same stale-key hazard as above: a server
                    // rejection is the signal that the on-disk file
                    // may have been replaced, so re-read it on the
                    // next attempt.
                    *self.key_cache.lock().expect("key cache poison") = None;
                    return Err(SyncError::auth("SSH key authentication rejected by server"));
                }
            }
        }

        // Auth succeeded → safe to commit a TOFU record. Doing
        // this AFTER auth ensures we don't pin a key we never
        // actually used (e.g. wrong password against a hostile
        // MitM would still record their key under our hostname
        // before failing auth).
        if let Some(HandshakeOutcome {
            decision: HostKeyDecision::AcceptAndRemember,
            fingerprint,
        }) = &recorded_outcome
        {
            self.host_key_verifier.record(&host_port, fingerprint);
        }

        let channel = handle
            .channel_open_session()
            .await
            .map_err(|err| SyncError::network(format!("open session channel: {err}")))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|err| SyncError::network(format!("request sftp subsystem: {err}")))?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|err| SyncError::network(format!("sftp init: {err}")))?;
        Ok(Arc::new(SessionPair {
            _handle: handle,
            sftp,
        }))
    }

    /// Load + parse the private key once per adapter lifetime.
    /// Held under the lock for the whole load so a concurrent
    /// connect can't run the slow KDF twice; in practice connects
    /// are serialized by the session mutex anyway.
    fn cached_private_key(
        &self,
        path: &Path,
        passphrase: Option<&str>,
    ) -> SyncResult<Arc<russh::keys::PrivateKey>> {
        let mut slot = self.key_cache.lock().expect("key cache poison");
        if let Some(key) = slot.as_ref() {
            return Ok(Arc::clone(key));
        }
        let key = Arc::new(load_private_key(path, passphrase)?);
        *slot = Some(Arc::clone(&key));
        Ok(key)
    }

    /// Run `op` against the cached live session, connecting first if
    /// there isn't one yet.
    ///
    /// Reuse-with-one-retry semantics: a cached session can have died
    /// idle between rounds (NAT boxes / home routers reap quiet TCP
    /// flows — the SFTP analog of webdav's stale-pooled-socket
    /// hazard), and russh only notices at the next operation. So an
    /// error on a REUSED session drops it and reruns the whole `op`
    /// once on a fresh connection. An error on a fresh connection is
    /// a real failure and propagates; either way the failed session
    /// is discarded so the next call starts clean. `op` must
    /// therefore be safe to run twice — every adapter operation is
    /// (reads are pure, writes are idempotent per the trait
    /// contract).
    async fn with_session<'a, T, F>(&'a self, op: F) -> SyncResult<T>
    where
        F: Fn(Arc<SessionPair>) -> BoxFuture<'a, SyncResult<T>>,
    {
        let mut slot = self.session.lock().await;
        let (reused, session) = match slot.take() {
            Some(live) => (true, live),
            None => (false, self.connect().await?),
        };
        match op(Arc::clone(&session)).await {
            Ok(value) => {
                *slot = Some(session);
                Ok(value)
            }
            Err(err) if reused => {
                debug!(
                    ?err,
                    "operation failed on a reused SSH session; reconnecting once",
                );
                drop(session);
                let fresh = self.connect().await?;
                match op(Arc::clone(&fresh)).await {
                    Ok(value) => {
                        *slot = Some(fresh);
                        Ok(value)
                    }
                    Err(err) => Err(err),
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Create the directory chain for `path` at most once per
    /// adapter session (webdav's `ensure_collection`, ported).
    /// Directories persist server-side, so re-walking `mkdir_p`
    /// before every push wastes one round trip per path component —
    /// an absolute base like `/home/alice/aperio/assets/sounds`
    /// cost ~5 `create_dir`s per sound asset without the cache. A
    /// concurrent first touch at worst issues a second harmless
    /// mkdir walk.
    async fn ensure_dir(&self, sftp: &SftpSession, path: &str) -> SyncResult<()> {
        if self
            .ensured
            .lock()
            .expect("ensured mutex poison")
            .contains(path)
        {
            return Ok(());
        }
        self.mkdir_p(sftp, path).await?;
        // mkdir_p swallows every per-component error (already-
        // exists is indistinguishable from a real failure at that
        // layer), so stat the deepest directory before caching —
        // it is the only proof the walk didn't silently no-op on
        // an unwritable path or a dead session. Without it a
        // failed walk would poison the cache, and test_connection
        // (which routes through here) would report Ok on a base
        // path the SSH account can't create. One extra round trip,
        // paid only on the first ensure per path per session.
        sftp.metadata(path).await.map_err(|err| {
            SyncError::network(format!(
                "directory {path} missing after create — check path/permissions: {err}"
            ))
        })?;
        self.ensured
            .lock()
            .expect("ensured mutex poison")
            .insert(path.to_string());
        Ok(())
    }

    async fn mkdir_p(&self, sftp: &SftpSession, path: &str) -> SyncResult<()> {
        // SFTP doesn't have a recursive mkdir; walk the
        // components creating each in turn. `create_dir` returns
        // an error if the directory already exists, which we
        // tolerate by ignoring all errors here — `ensure_dir`'s
        // post-walk stat (or the subsequent open/write) fails
        // loudly if the directory truly couldn't be created.
        let mut current = String::new();
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            current.push('/');
            current.push_str(segment);
            let _ = sftp.create_dir(&current).await;
        }
        Ok(())
    }
}

/// SSH client handler. Consults the verifier configured on the
/// `SftpSyncAdapter`; writes its verdict + the observed
/// fingerprint into the side-channel slot the connect helper
/// reads after the handshake.
///
/// `russh::client::Handler` uses native `async fn` in trait
/// (Rust 1.75+), so we do NOT mark the impl with
/// `#[async_trait]` — the attribute would rewrite the lifetime
/// annotations away from the trait's signature.
#[derive(Clone)]
struct ClientHandler {
    verifier: Arc<dyn HostKeyVerifier>,
    /// `"host:port"` lookup key for the verifier.
    host_port: String,
    /// Side-channel for the connect helper. Set during
    /// `check_server_key`; read after the handshake (either to
    /// surface a mismatch error or to commit a TOFU record).
    outcome: Arc<Mutex<Option<HandshakeOutcome>>>,
}

impl std::fmt::Debug for ClientHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClientHandler")
            .field("host_port", &self.host_port)
            .finish()
    }
}

/// Carried through the connect side-channel.
#[derive(Debug, Clone)]
struct HandshakeOutcome {
    decision: HostKeyDecision,
    /// SHA256 fingerprint of the presented key — needed by the
    /// connect helper to call `record()` on TOFU acceptance.
    fingerprint: String,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        let decision = self.verifier.verify(&self.host_port, &fingerprint);
        let accept = !matches!(decision, HostKeyDecision::Mismatch { .. });
        *self.outcome.lock().expect("handshake outcome poison") = Some(HandshakeOutcome {
            decision,
            fingerprint,
        });
        Ok(accept)
    }
}

/// One-shot handler used by [`SftpSyncAdapter::probe_host_key_fingerprint`]
/// to capture the server's SHA256 fingerprint without performing
/// SSH authentication. Writes the fingerprint into a shared
/// `Mutex<Option<String>>` slot, then returns `Ok(false)` so russh
/// aborts the handshake cleanly — we never go past the host-key
/// step.
#[derive(Clone)]
struct ProbeHandler {
    captured: Arc<Mutex<Option<String>>>,
}

impl std::fmt::Debug for ProbeHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProbeHandler").finish()
    }
}

impl client::Handler for ProbeHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();
        *self.captured.lock().expect("probe capture poison") = Some(fingerprint);
        // Abort the handshake — we have what we came for.
        Ok(false)
    }
}

/// Load a private key from disk, optionally decrypting it with
/// `passphrase`. Handles both PEM and OpenSSH-format keys via
/// russh's helper. Errors map to `SyncError::Auth` so the UI
/// can show a "key file unreadable / wrong passphrase" message
/// instead of a generic IO error.
fn load_private_key(path: &Path, passphrase: Option<&str>) -> SyncResult<russh::keys::PrivateKey> {
    russh::keys::load_secret_key(path, passphrase).map_err(|err| {
        SyncError::auth(format!(
            "couldn't load SSH key at {}: {err}",
            path.display(),
        ))
    })
}

/// Inline copy of russh's private `AlgorithmExt::hash_alg`.
/// Returns `Some(HashAlg)` only for RSA keys; every other
/// algorithm uses its built-in hash (Ed25519 / ECDSA / etc.).
fn hash_alg_for(alg: &Algorithm) -> Option<HashAlg> {
    match alg {
        Algorithm::Rsa { hash } => *hash,
        _ => None,
    }
}

/// Bounded per-file read concurrency inside one `fetch_new_logs`.
/// Matches webdav's value; modest so shy servers (per-session
/// request caps, rate limits) aren't hammered.
const LOG_FETCH_CONCURRENCY: usize = 4;

/// Decide which listed log files `fetch_new_logs` should read.
///
/// Pure so the growth-refetch semantics are unit-testable without
/// an SSH server: (filename, listed size) pairs from the READDIR
/// reply in, parsed names the cursor wants out. The size feeds
/// `DeviceCursor::wants_sized`, which re-fetches a peer's live
/// session file that gained appended events even though its
/// timestamp sits at/below the cursor — the append-miss data-loss
/// class. Sizes are reported RAW (remote bytes): on an E2E dataset
/// they're ciphertext lengths, and the `EncryptingAdapter` has
/// already translated the cursor's `known_lengths` into that
/// domain before this adapter sees them. Never adjust them here.
fn select_wanted_logs(
    entries: impl IntoIterator<Item = (String, Option<u64>)>,
    since: &DeviceCursor,
) -> Vec<LogFileName> {
    let mut wanted = Vec::new();
    for (name, listed_len) in entries {
        let parsed = match LogFileName::from_filename(&name) {
            Ok(p) => p,
            Err(_) => {
                debug!(name = %name, "skipping non-log entry in read_dir");
                continue;
            }
        };
        if since.wants_sized(&parsed, &name, listed_len) {
            wanted.push(parsed);
        }
    }
    wanted
}

/// What `fetch_new_logs` does with a failed per-file open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FetchErrorDisposition {
    /// The compactor deleted the file between READDIR and open —
    /// the listing was merely stale. Skip the file silently; the
    /// next round lists fresh.
    SkipNotFoundRace,
    /// Anything else fails the WHOLE fetch. Returning the rest of
    /// the batch while dropping one file would let the orchestrator
    /// advance the cursor past it — the file falls below the cursor
    /// with no applied-length record, and its events are
    /// permanently lost to this device. Failing keeps the cursor
    /// put; the caller serves stale and retries next round (webdav
    /// semantics).
    FailBatch,
}

/// The single place deciding skip-vs-fail for a per-file fetch
/// error, so the not-found race stays the ONLY silent skip.
fn fetch_error_disposition(err: &russh_sftp::client::error::Error) -> FetchErrorDisposition {
    if is_not_found(err) {
        FetchErrorDisposition::SkipNotFoundRace
    } else {
        FetchErrorDisposition::FailBatch
    }
}

/// Adapter-agnostic skeleton of `fetch_new_logs`: thread the
/// listed `(filename, listed size)` pairs through
/// [`select_wanted_logs`], read the survivors with bounded
/// concurrency, and route every per-file error through
/// `disposition`. Generic over the fetch closure + error type so
/// the WIRING — sizes actually reaching the growth check, errors
/// actually reaching the disposition — is unit-testable without
/// an SSH server; the trait method is a thin instantiation whose
/// only untested residue is the SFTP glue (attribute extraction
/// plus the open/read closure).
async fn fetch_logs_via<E, F, Fut>(
    entries: impl IntoIterator<Item = (String, Option<u64>)>,
    since: &DeviceCursor,
    disposition: impl Fn(&E) -> FetchErrorDisposition,
    fail_batch: impl Fn(&LogFileName, &E) -> SyncError,
    fetch_one: F,
) -> SyncResult<Vec<LogFile>>
where
    F: Fn(String) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>, E>>,
{
    let wanted = select_wanted_logs(entries, since);
    // Bounded concurrency: a multi-file backlog (onboarding,
    // post-offline catch-up) used to pay one serial round trip
    // per file. Unordered completion is fine — the orchestrator
    // sorts fetched logs chronologically before apply.
    let results: Vec<SyncResult<Option<LogFile>>> = stream::iter(wanted)
        .map(|parsed| {
            let disposition = &disposition;
            let fail_batch = &fail_batch;
            // Calling `fetch_one` here only BUILDS the future;
            // nothing runs until buffer_unordered polls it, so
            // the concurrency bound holds.
            let fut = fetch_one(parsed.to_filename());
            async move {
                match fut.await {
                    Ok(bytes) => Ok(Some(LogFile {
                        name: parsed,
                        bytes,
                    })),
                    Err(err) => match disposition(&err) {
                        FetchErrorDisposition::SkipNotFoundRace => {
                            debug!(
                                name = %parsed.to_filename(),
                                "log file listed but no longer present",
                            );
                            Ok(None)
                        }
                        FetchErrorDisposition::FailBatch => Err(fail_batch(&parsed, &err)),
                    },
                }
            }
        })
        .buffer_unordered(LOG_FETCH_CONCURRENCY)
        .collect()
        .await;
    let mut out = Vec::with_capacity(results.len());
    for result in results {
        if let Some(log) = result? {
            out.push(log);
        }
    }
    Ok(out)
}

#[async_trait]
impl SyncAdapter for SftpSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        self.with_session(|session| {
            Box::pin(async move {
                // The Connect button is an explicit probe — drop the
                // ensured cache first so a wiped server gets its tree
                // re-created rather than assumed intact (mirrors the
                // FTP adapter).
                self.ensured.lock().expect("ensured mutex poison").clear();
                // Probe by stat'ing the base path. If it doesn't
                // exist yet, try to create it + the sub-collections;
                // if THAT fails the user picked a path the SSH
                // account can't write to.
                let base = self.remote_path("");
                if session.sftp.metadata(&base).await.is_err() {
                    self.mkdir_p(&session.sftp, &base).await?;
                }
                // Lazy-create log/ + assets/sounds/ so first-push
                // works — and seed the ensured cache so those pushes
                // skip their own mkdir walks.
                self.ensure_dir(&session.sftp, &self.remote_path("log"))
                    .await?;
                self.ensure_dir(&session.sftp, &self.remote_path("assets/sounds"))
                    .await?;
                Ok(())
            })
        })
        .await
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        self.with_session(|session| {
            Box::pin(async move {
                let path = self.remote_path("meta.json");
                match session.sftp.open(&path).await {
                    Ok(mut file) => {
                        let mut bytes = Vec::new();
                        file.read_to_end(&mut bytes)
                            .await
                            .map_err(|err| SyncError::network(format!("read meta.json: {err}")))?;
                        Ok(Some(MetaJson::from_bytes(&bytes)?))
                    }
                    Err(err) if is_not_found(&err) => Ok(None),
                    Err(err) => Err(SyncError::network(format!("open meta.json: {err}"))),
                }
            })
        })
        .await
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let bytes = meta.to_bytes()?;
        self.with_session(|session| {
            let bytes = &bytes;
            Box::pin(async move {
                atomic_write(&session.sftp, &self.remote_path("meta.json"), bytes).await
            })
        })
        .await
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        self.with_session(|session| {
            Box::pin(async move {
                let log_dir = self.remote_path("log");
                let entries = match session.sftp.read_dir(&log_dir).await {
                    Ok(e) => e,
                    Err(err) if is_not_found(&err) => return Ok(Vec::new()),
                    Err(err) => {
                        return Err(SyncError::network(format!("read_dir log/: {err}")));
                    }
                };

                // The SSH_FXP_READDIR reply already carries each
                // entry's attributes — `DirEntry::metadata()` is a
                // synchronous accessor, so feeding sizes into the
                // growth check costs zero extra round trips. The
                // concurrent opens/reads inside the skeleton share
                // the one session (the protocol multiplexes request
                // ids over the channel) — no extra connections.
                let log_dir = &log_dir;
                let sftp = &session.sftp;
                fetch_logs_via(
                    entries.map(|entry| {
                        let listed_len = entry.metadata().size;
                        (entry.file_name(), listed_len)
                    }),
                    since,
                    fetch_error_disposition,
                    |name, err| {
                        SyncError::network(format!(
                            "fetch log {log_dir}/{}: {err}",
                            name.to_filename(),
                        ))
                    },
                    |filename| async move {
                        let path = format!("{log_dir}/{filename}");
                        let mut file = sftp.open(&path).await?;
                        let mut bytes = Vec::new();
                        // The read error (io::Error) converts into
                        // the client error's IO variant — never a
                        // Status, so read failures always fail the
                        // batch, exactly as before the extraction.
                        file.read_to_end(&mut bytes).await?;
                        Ok(bytes)
                    },
                )
                .await
            })
        })
        .await
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        self.with_session(|session| {
            Box::pin(async move {
                // Ensure log/ exists — once per adapter session, not
                // per push (a startup backlog of N pending logs used
                // to pay N redundant create_dir round trips).
                self.ensure_dir(&session.sftp, &self.remote_path("log"))
                    .await?;
                let path = self.remote_path(&format!("log/{}", log.name.to_filename()));
                write_file(&session.sftp, &path, &log.bytes).await
            })
        })
        .await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        self.with_session(|session| {
            Box::pin(async move {
                let path = self.remote_path("snapshot.json");
                match session.sftp.open(&path).await {
                    Ok(mut file) => {
                        let mut bytes = Vec::new();
                        file.read_to_end(&mut bytes).await.map_err(|err| {
                            SyncError::network(format!("read snapshot.json: {err}"))
                        })?;
                        Ok(Some(Snapshot::from_bytes(&bytes)?))
                    }
                    Err(err) if is_not_found(&err) => Ok(None),
                    Err(err) => Err(SyncError::network(format!("open snapshot.json: {err}"))),
                }
            })
        })
        .await
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let bytes = snapshot.to_bytes()?;
        self.with_session(|session| {
            let bytes = &bytes;
            Box::pin(async move {
                atomic_write(&session.sftp, &self.remote_path("snapshot.json"), bytes).await
            })
        })
        .await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        self.with_session(|session| {
            Box::pin(async move {
                let path = self.remote_path(&format!("log/{}", name.to_filename()));
                match session.sftp.remove_file(&path).await {
                    Ok(()) => Ok(()),
                    // Not-found is treated as success — the goal is
                    // "make sure it's gone", and absent already
                    // satisfies that.
                    Err(err) if is_not_found(&err) => Ok(()),
                    Err(err) => Err(SyncError::network(format!("delete {path}: {err}"))),
                }
            })
        })
        .await
    }

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        self.with_session(|session| {
            Box::pin(async move {
                // The full mkdir_p walk used to run on EVERY asset
                // push (~one create_dir per path component); the
                // ensured cache reduces that to once per session.
                self.ensure_dir(&session.sftp, &self.remote_path("assets/sounds"))
                    .await?;
                let path = self.remote_path(&format!("assets/sounds/{hash}.{extension}"));
                write_file(&session.sftp, &path, bytes).await
            })
        })
        .await
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        self.with_session(|session| {
            Box::pin(async move {
                let path = self.remote_path(&format!("assets/sounds/{hash}.{extension}"));
                match session.sftp.open(&path).await {
                    Ok(mut file) => {
                        let mut out = Vec::new();
                        file.read_to_end(&mut out).await.map_err(|err| {
                            SyncError::network(format!("read sound asset: {err}"))
                        })?;
                        Ok(Some(out))
                    }
                    Err(err) if is_not_found(&err) => Ok(None),
                    Err(err) => Err(SyncError::network(format!("open sound asset: {err}"))),
                }
            })
        })
        .await
    }
}

/// Write bytes to `path`. Creates / truncates / writes / closes.
async fn write_file(sftp: &SftpSession, path: &str, bytes: &[u8]) -> SyncResult<()> {
    let mut file = sftp
        .open_with_flags(
            path,
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|err| SyncError::network(format!("create {path}: {err}")))?;
    file.write_all(bytes)
        .await
        .map_err(|err| SyncError::network(format!("write {path}: {err}")))?;
    file.shutdown()
        .await
        .map_err(|err| SyncError::network(format!("close {path}: {err}")))?;
    Ok(())
}

/// Atomic write via temp + rename. Used for meta.json + snapshot.json
/// so a crash mid-write can't leave a corrupt control file.
async fn atomic_write(sftp: &SftpSession, path: &str, bytes: &[u8]) -> SyncResult<()> {
    let tmp = format!("{path}.tmp");
    write_file(sftp, &tmp, bytes).await?;
    // POSIX rename is atomic on the same filesystem. SFTP's
    // `rename` translates straight to the server's syscall.
    // Some servers reject rename-over-existing; remove + rename
    // is the portable fallback. Try plain rename first.
    if let Err(err) = sftp.rename(&tmp, path).await {
        debug!(?err, "rename failed; falling back to remove + rename");
        let _ = sftp.remove_file(path).await;
        sftp.rename(&tmp, path)
            .await
            .map_err(|err| SyncError::network(format!("rename {tmp} → {path}: {err}")))?;
    }
    Ok(())
}

/// Detect SFTP's "file not found" status code. russh-sftp surfaces
/// it as `StatusCode::NoSuchFile` inside a typed `Error::Status`
/// variant; pattern-matching on the typed value is more reliable
/// than substring matching on the display form.
fn is_not_found(err: &russh_sftp::client::error::Error) -> bool {
    use russh_sftp::client::error::Error;
    matches!(
        err,
        Error::Status(s) if s.status_code == russh_sftp::protocol::StatusCode::NoSuchFile,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use sync_core::{DeviceId, KnownLogLength};

    fn log_name(ts_secs: i64, device: &str) -> LogFileName {
        LogFileName::new(
            Utc.timestamp_opt(ts_secs, 0).unwrap(),
            DeviceId::from_string(device.into()),
        )
    }

    /// Build a typed SFTP status error — the shape russh-sftp
    /// surfaces server NoSuchFile / PermissionDenied replies in.
    fn sftp_status_err(code: russh_sftp::protocol::StatusCode) -> russh_sftp::client::error::Error {
        russh_sftp::client::error::Error::Status(russh_sftp::protocol::Status {
            id: 0,
            status_code: code,
            error_message: String::new(),
            language_tag: String::new(),
        })
    }

    // -----------------------------------------------------------------
    // Wanted-selection (growth refetch) tests — pure helper, no SSH
    // -----------------------------------------------------------------

    #[test]
    fn grown_file_at_the_cursor_is_refetched() {
        // The append-miss fix (mirrors adapter-local's test of
        // the same name): a peer's live session file gains events
        // AFTER we applied it; its timestamp sits AT the cursor, but
        // the listed READDIR size exceeds the recorded applied
        // length, so it must be fetched again.
        let file = log_name(1_000, "dev-a");
        let filename = file.to_filename();
        let cursor_at = |known_len: u64| DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: vec![KnownLogLength {
                name: filename.clone(),
                len: known_len,
            }],
        };

        // Listed size grew past the applied length → refetched.
        let wanted = select_wanted_logs([(filename.clone(), Some(150))], &cursor_at(100));
        assert_eq!(wanted, vec![file.clone()], "grown file re-fetched");

        // Listed size equals the applied length → unchanged → skipped.
        assert!(
            select_wanted_logs([(filename.clone(), Some(100))], &cursor_at(100)).is_empty(),
            "unchanged file skipped",
        );

        // No listed size (server omitted the attribute) → plain
        // cursor semantics → skipped.
        assert!(
            select_wanted_logs([(filename.clone(), None)], &cursor_at(100)).is_empty(),
            "sizeless listing never growth-refetches",
        );
    }

    #[test]
    fn selection_applies_cursor_exclusion_and_name_parsing() {
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: Vec::new(),
        };
        let wanted = select_wanted_logs(
            [
                // Newer + foreign → fetched, size or not.
                (log_name(2_000, "peer").to_filename(), None),
                // Newer but OWN → skipped at the listing stage.
                (log_name(2_000, "me").to_filename(), Some(10)),
                // Older foreign, no growth record → below the horizon.
                (log_name(500, "peer").to_filename(), Some(10)),
                // Non-log names (temp files, editor backups) → skipped.
                (".snapshot.json.tmp".to_string(), Some(10)),
            ],
            &cursor,
        );
        assert_eq!(wanted, vec![log_name(2_000, "peer")]);
    }

    // -----------------------------------------------------------------
    // Per-file fetch-error disposition tests
    // -----------------------------------------------------------------

    #[test]
    fn fetch_error_disposition_skips_only_the_not_found_race() {
        use russh_sftp::protocol::StatusCode;
        // Compactor deleted the file between READDIR and open → the
        // listing was stale; skip silently.
        assert_eq!(
            fetch_error_disposition(&sftp_status_err(StatusCode::NoSuchFile)),
            FetchErrorDisposition::SkipNotFoundRace,
        );
        // EVERYTHING else fails the whole batch — a silently skipped
        // file would fall below the advancing cursor and lose its
        // events forever.
        assert_eq!(
            fetch_error_disposition(&sftp_status_err(StatusCode::PermissionDenied)),
            FetchErrorDisposition::FailBatch,
        );
        assert_eq!(
            fetch_error_disposition(&russh_sftp::client::error::Error::Timeout),
            FetchErrorDisposition::FailBatch,
        );
    }

    // -----------------------------------------------------------------
    // Fetch-skeleton wiring tests — fake fetch closure, no SSH.
    // These pin what the pure-helper tests above cannot: that the
    // listing→selection→per-file-fetch pipeline actually threads
    // names + sizes into `select_wanted_logs` and routes errors
    // through `fetch_error_disposition`.
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn fetch_skeleton_threads_names_and_sizes_into_selection() {
        // Wiring twin of `grown_file_at_the_cursor_is_refetched`:
        // if a refactor dropped the size plumbing (listed sizes →
        // None), the grown file would be skipped and this fails —
        // the append-miss data-loss class silently reopening.
        let grown = log_name(1_000, "dev-a");
        let grown_name = grown.to_filename();
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: vec![KnownLogLength {
                name: grown_name.clone(),
                len: 100,
            }],
        };
        let fetched = std::sync::Mutex::new(Vec::new());
        let logs = fetch_logs_via(
            [
                // Grew past the applied length → wanted.
                (grown_name.clone(), Some(150)),
                // At the cursor without a growth record → skipped.
                // Only the listed sizes tell these two apart.
                (log_name(1_000, "dev-b").to_filename(), Some(80)),
            ],
            &cursor,
            fetch_error_disposition,
            |name, err| SyncError::network(format!("{}: {err}", name.to_filename())),
            |filename| {
                fetched.lock().expect("fetched poison").push(filename);
                async { Ok(b"grown".to_vec()) }
            },
        )
        .await
        .expect("fetch succeeds");
        assert_eq!(
            *fetched.lock().expect("fetched poison"),
            vec![grown_name],
            "exactly the grown file is fetched, by its filename",
        );
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].name, grown);
        assert_eq!(logs[0].bytes, b"grown");
    }

    #[tokio::test]
    async fn fetch_skeleton_skips_not_found_race_and_returns_rest() {
        // A file the compactor deleted between READDIR and open is
        // silently skipped; the rest of the batch still comes back.
        use russh_sftp::protocol::StatusCode;
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: Vec::new(),
        };
        let kept = log_name(2_000, "dev-a");
        let vanished_name = log_name(3_000, "dev-b").to_filename();
        let logs = fetch_logs_via(
            [
                (kept.to_filename(), Some(10)),
                (vanished_name.clone(), Some(10)),
            ],
            &cursor,
            fetch_error_disposition,
            |name, err| SyncError::network(format!("{}: {err}", name.to_filename())),
            |filename| {
                let vanished = filename == vanished_name;
                async move {
                    if vanished {
                        Err(sftp_status_err(StatusCode::NoSuchFile))
                    } else {
                        Ok(b"kept".to_vec())
                    }
                }
            },
        )
        .await
        .expect("the not-found race must not fail the batch");
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].name, kept);
    }

    #[tokio::test]
    async fn fetch_skeleton_fails_whole_batch_on_other_errors() {
        // Any non-not-found per-file error fails the WHOLE fetch —
        // returning the rest would let the orchestrator advance the
        // cursor past the broken file and lose its events forever.
        use russh_sftp::protocol::StatusCode;
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: Vec::new(),
        };
        let broken_name = log_name(3_000, "dev-b").to_filename();
        let err = fetch_logs_via(
            [
                (log_name(2_000, "dev-a").to_filename(), Some(10)),
                (broken_name.clone(), Some(10)),
            ],
            &cursor,
            fetch_error_disposition,
            |name, err| SyncError::network(format!("{}: {err}", name.to_filename())),
            |filename| {
                let broken = filename == broken_name;
                async move {
                    if broken {
                        Err(sftp_status_err(StatusCode::PermissionDenied))
                    } else {
                        Ok(Vec::new())
                    }
                }
            },
        )
        .await
        .expect_err("a permission error must fail the batch");
        assert!(
            err.to_string().contains(&broken_name),
            "batch error names the broken file: {err}",
        );
    }

    #[test]
    fn remote_path_joins_relative_segments() {
        let a =
            SftpSyncAdapter::new_password("h", 22, "u", "p", PathBuf::from("/home/alice/aperio"));
        assert_eq!(a.remote_path("meta.json"), "/home/alice/aperio/meta.json",);
        assert_eq!(
            a.remote_path("log/2026-05-01T08-00-00Z_dev-a.jsonl"),
            "/home/alice/aperio/log/2026-05-01T08-00-00Z_dev-a.jsonl",
        );
    }

    #[test]
    fn remote_path_trims_redundant_slashes() {
        let a =
            SftpSyncAdapter::new_password("h", 22, "u", "p", PathBuf::from("/home/alice/aperio/"));
        assert_eq!(a.remote_path("/meta.json"), "/home/alice/aperio/meta.json",);
        // Empty relative returns the base.
        assert_eq!(a.remote_path(""), "/home/alice/aperio");
    }

    #[test]
    fn remote_path_normalises_windows_backslashes_in_base() {
        // On Windows the PathBuf may carry backslashes; SFTP
        // demands forward slashes server-side.
        let a = SftpSyncAdapter::new_password(
            "h",
            22,
            "u",
            "p",
            PathBuf::from("\\home\\alice\\aperio"),
        );
        assert_eq!(a.remote_path("meta.json"), "/home/alice/aperio/meta.json");
    }

    // -----------------------------------------------------------------
    // HostKeyVerifier tests
    // -----------------------------------------------------------------

    #[test]
    fn in_memory_verifier_first_use_returns_accept_and_remember() {
        let v = InMemoryHostKeyVerifier::new();
        let d = v.verify("nas:22", "SHA256:abc");
        assert_eq!(d, HostKeyDecision::AcceptAndRemember);
    }

    #[test]
    fn in_memory_verifier_known_key_returns_accept() {
        let v = InMemoryHostKeyVerifier::with_known("nas:22", "SHA256:abc");
        let d = v.verify("nas:22", "SHA256:abc");
        assert_eq!(d, HostKeyDecision::Accept);
    }

    #[test]
    fn in_memory_verifier_changed_key_returns_mismatch() {
        let v = InMemoryHostKeyVerifier::with_known("nas:22", "SHA256:abc");
        let d = v.verify("nas:22", "SHA256:zzz");
        assert_eq!(
            d,
            HostKeyDecision::Mismatch {
                stored: "SHA256:abc".into(),
                presented: "SHA256:zzz".into(),
            },
        );
    }

    #[test]
    fn in_memory_verifier_record_persists_across_verify_calls() {
        // Implement the TOFU flow manually: verify returns
        // AcceptAndRemember on first use; we commit via record;
        // next verify returns Accept.
        let v = InMemoryHostKeyVerifier::new();
        assert_eq!(
            v.verify("nas:22", "SHA256:abc"),
            HostKeyDecision::AcceptAndRemember,
        );
        v.record("nas:22", "SHA256:abc");
        assert_eq!(v.verify("nas:22", "SHA256:abc"), HostKeyDecision::Accept,);
    }

    #[test]
    fn in_memory_verifier_forget_drops_pin() {
        let v = InMemoryHostKeyVerifier::with_known("nas:22", "SHA256:abc");
        v.forget("nas:22");
        // After forget, verify returns AcceptAndRemember again —
        // the host is treated as new on next contact.
        assert_eq!(
            v.verify("nas:22", "SHA256:zzz"),
            HostKeyDecision::AcceptAndRemember,
        );
    }

    #[test]
    fn in_memory_verifier_forget_unknown_is_noop() {
        let v = InMemoryHostKeyVerifier::new();
        // Doesn't panic when nothing is pinned.
        v.forget("nas:22");
        assert_eq!(v.peek("nas:22"), None);
    }

    #[test]
    fn in_memory_verifier_record_overwrites_existing() {
        // A user that explicitly accepts a changed key (via the
        // §19.5 "verify out-of-band, then re-pin" flow) calls
        // record() with the new fingerprint; the next verify
        // returns Accept.
        let v = InMemoryHostKeyVerifier::with_known("nas:22", "SHA256:old");
        v.record("nas:22", "SHA256:new");
        assert_eq!(v.verify("nas:22", "SHA256:new"), HostKeyDecision::Accept,);
    }
}
