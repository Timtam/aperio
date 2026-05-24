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
//! v1 opens a fresh SSH session per `SyncAdapter` method call.
//! Sync rounds run every 5 minutes by default; the connect cost
//! (≈100-300 ms for SSH handshake) is dwarfed by the cost of the
//! actual transfer. We can switch to a long-lived pooled
//! connection later if profiling shows it matters.
//!
//! ## Host-key verification (v1 contract)
//!
//! `ClientHandler::check_server_key` accepts **any** host key.
//! That's the lowest-friction path for v1 and matches how most
//! WebDAV / FTPS setups work (TLS cert pinning is also opt-in).
//! A Phase-Sm follow-up wires a TOFU (trust-on-first-use) prompt
//! that stores the fingerprint in `user_prefs` and rejects
//! changes.
//!
//! ## Auth
//!
//! v1 supports **password** auth only. SSH key files (PEM or
//! OpenSSH format, with or without passphrase) land in a follow-
//! up — the russh API already exposes `authenticate_publickey`,
//! but the UX of picking + storing a key file is its own scope.
//!
//! ## What this crate does NOT do
//!
//! - **TOFU host-key pinning.** v1 accepts any key.
//! - **SSH-key authentication.** v1 password-only.
//! - **Connection pooling.** v1 per-operation connect.
//! - **Resume / partial uploads.** Each write is one round-trip;
//!   meta.json + snapshot.json use atomic write-temp + rename so
//!   a crash mid-write can't leave a corrupt control file. Log
//!   files are write-once with timestamp + device id in the
//!   name, so a partial write is retried naturally by the
//!   scheduler picking up the same pending file.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use russh::client::{self, Handle};
use russh::keys::ssh_key::PublicKey;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
    SyncError, SyncResult,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, warn};

/// SFTP adapter configuration.
///
/// `host` is bare ("nas.example.com"); the adapter prepends
/// `port` at connect time. `base_path` is an absolute path on the
/// remote host (e.g. `/home/alice/aperio`) — Aperio's log/, meta,
/// snapshot, and assets/ live directly under it.
#[derive(Debug, Clone)]
pub struct SftpSyncAdapter {
    host: String,
    port: u16,
    user: String,
    password: String,
    base_path: PathBuf,
}

impl SftpSyncAdapter {
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        password: impl Into<String>,
        base_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            base_path: base_path.into(),
        }
    }

    /// Borrow the configured base path. Used by Settings for the
    /// "current adapter: sftp://…" display.
    pub fn base_path(&self) -> &std::path::Path {
        &self.base_path
    }

    /// Concatenate the base path with a relative segment using
    /// forward slashes — SFTP paths are always POSIX-style even
    /// when the local OS is Windows.
    fn remote_path(&self, relative: &str) -> String {
        let base = self
            .base_path
            .to_string_lossy()
            .replace('\\', "/");
        let trimmed_base = base.trim_end_matches('/');
        let trimmed_rel = relative.trim_start_matches('/');
        if trimmed_rel.is_empty() {
            trimmed_base.to_string()
        } else {
            format!("{trimmed_base}/{trimmed_rel}")
        }
    }

    /// Open a fresh SSH + SFTP session. Caller owns the returned
    /// handles; dropping them closes the connection.
    ///
    /// Returns the SSH handle alongside the `SftpSession` so the
    /// caller keeps the handle alive — dropping the handle while
    /// using the session aborts mid-operation.
    async fn connect(&self) -> SyncResult<(Handle<ClientHandler>, SftpSession)> {
        let config = Arc::new(client::Config::default());
        let mut handle = client::connect(
            config,
            (self.host.as_str(), self.port),
            ClientHandler,
        )
        .await
        .map_err(|err| SyncError::network(format!("ssh connect: {err}")))?;
        let authed = handle
            .authenticate_password(self.user.as_str(), self.password.as_str())
            .await
            .map_err(|err| SyncError::auth(format!("ssh auth: {err}")))?;
        if !authed.success() {
            return Err(SyncError::auth(
                "SSH password authentication rejected by server",
            ));
        }
        let channel = handle
            .channel_open_session()
            .await
            .map_err(|err| {
                SyncError::network(format!("open session channel: {err}"))
            })?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|err| {
                SyncError::network(format!("request sftp subsystem: {err}"))
            })?;
        let sftp = SftpSession::new(channel.into_stream()).await.map_err(
            |err| SyncError::network(format!("sftp init: {err}")),
        )?;
        Ok((handle, sftp))
    }

    async fn mkdir_p(&self, sftp: &SftpSession, path: &str) -> SyncResult<()> {
        // SFTP doesn't have a recursive mkdir; walk the
        // components creating each in turn. `create_dir` returns
        // an error if the directory already exists, which we
        // tolerate by ignoring all errors here — the subsequent
        // open/write will fail loudly if the directory truly
        // couldn't be created.
        let mut current = String::new();
        for segment in path.split('/').filter(|s| !s.is_empty()) {
            current.push('/');
            current.push_str(segment);
            let _ = sftp.create_dir(&current).await;
        }
        Ok(())
    }
}

/// SSH client handler. v1 accepts every host key — see the
/// module docs for the trade-off + the follow-up plan.
///
/// `russh::client::Handler` uses native `async fn` in trait
/// (Rust 1.75+), so we do NOT mark the impl with
/// `#[async_trait]` — the attribute would rewrite the lifetime
/// annotations away from the trait's signature.
#[derive(Debug, Clone, Default)]
struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept every key. Real production = TOFU + fingerprint
        // pinning in user_prefs; that lands in a Phase-Sm follow-
        // up.
        Ok(true)
    }
}

#[async_trait]
impl SyncAdapter for SftpSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        let (_handle, sftp) = self.connect().await?;
        // Probe by stat'ing the base path. If it doesn't exist
        // yet, try to create it + the sub-collections; if THAT
        // fails the user picked a path the SSH account can't
        // write to.
        let base = self.remote_path("");
        if sftp.metadata(&base).await.is_err() {
            self.mkdir_p(&sftp, &base).await?;
        }
        // Lazy-create log/ + assets/sounds/ so first-push works.
        self.mkdir_p(&sftp, &self.remote_path("log")).await?;
        self.mkdir_p(&sftp, &self.remote_path("assets/sounds")).await?;
        Ok(())
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let (_handle, sftp) = self.connect().await?;
        let path = self.remote_path("meta.json");
        match sftp.open(&path).await {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).await.map_err(|err| {
                    SyncError::network(format!("read meta.json: {err}"))
                })?;
                Ok(Some(MetaJson::from_bytes(&bytes)?))
            }
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(SyncError::network(format!(
                "open meta.json: {err}"
            ))),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let (_handle, sftp) = self.connect().await?;
        let bytes = meta.to_bytes()?;
        atomic_write(&sftp, &self.remote_path("meta.json"), &bytes).await
    }

    async fn fetch_new_logs(
        &self,
        since: &DeviceCursor,
    ) -> SyncResult<Vec<LogFile>> {
        let (_handle, sftp) = self.connect().await?;
        let log_dir = self.remote_path("log");
        let entries = match sftp.read_dir(&log_dir).await {
            Ok(e) => e,
            Err(err) if is_not_found(&err) => return Ok(Vec::new()),
            Err(err) => {
                return Err(SyncError::network(format!(
                    "read_dir log/: {err}"
                )));
            }
        };

        let mut wanted: Vec<LogFileName> = Vec::new();
        for entry in entries {
            let name = entry.file_name();
            let parsed = match LogFileName::from_filename(&name) {
                Ok(p) => p,
                Err(_) => {
                    debug!(name = %name, "skipping non-log entry in read_dir");
                    continue;
                }
            };
            if parsed.timestamp > since.last_seen_log {
                wanted.push(parsed);
            }
        }

        let mut out = Vec::with_capacity(wanted.len());
        for parsed in wanted {
            let path = format!("{}/{}", log_dir, parsed.to_filename());
            match sftp.open(&path).await {
                Ok(mut file) => {
                    let mut bytes = Vec::new();
                    if let Err(err) = file.read_to_end(&mut bytes).await {
                        warn!(
                            path = %path,
                            ?err,
                            "read log file failed; skipping",
                        );
                        continue;
                    }
                    out.push(LogFile {
                        name: parsed,
                        bytes,
                    });
                }
                Err(err) if is_not_found(&err) => {
                    // Compactor raced us between read_dir + open;
                    // skip silently.
                    debug!(
                        path = %path,
                        "log file listed but no longer present",
                    );
                }
                Err(err) => {
                    warn!(
                        path = %path,
                        ?err,
                        "open log file failed; skipping",
                    );
                }
            }
        }
        Ok(out)
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let (_handle, sftp) = self.connect().await?;
        // Ensure log/ exists (cheap; mkdir on existing is
        // tolerated).
        let _ = sftp.create_dir(&self.remote_path("log")).await;
        let path = self.remote_path(&format!("log/{}", log.name.to_filename()));
        write_file(&sftp, &path, &log.bytes).await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let (_handle, sftp) = self.connect().await?;
        let path = self.remote_path("snapshot.json");
        match sftp.open(&path).await {
            Ok(mut file) => {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes).await.map_err(|err| {
                    SyncError::network(format!("read snapshot.json: {err}"))
                })?;
                Ok(Some(Snapshot::from_bytes(&bytes)?))
            }
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(SyncError::network(format!(
                "open snapshot.json: {err}"
            ))),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let (_handle, sftp) = self.connect().await?;
        let bytes = snapshot.to_bytes()?;
        atomic_write(&sftp, &self.remote_path("snapshot.json"), &bytes).await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let (_handle, sftp) = self.connect().await?;
        let path = self.remote_path(&format!("log/{}", name.to_filename()));
        match sftp.remove_file(&path).await {
            Ok(()) => Ok(()),
            // Not-found is treated as success — the goal is "make
            // sure it's gone", and absent already satisfies that.
            Err(err) if is_not_found(&err) => Ok(()),
            Err(err) => Err(SyncError::network(format!(
                "delete {path}: {err}"
            ))),
        }
    }

    async fn push_sound_asset(
        &self,
        hash: &str,
        extension: &str,
        bytes: &[u8],
    ) -> SyncResult<()> {
        let (_handle, sftp) = self.connect().await?;
        self.mkdir_p(&sftp, &self.remote_path("assets/sounds")).await?;
        let path = self
            .remote_path(&format!("assets/sounds/{hash}.{extension}"));
        write_file(&sftp, &path, bytes).await
    }

    async fn fetch_sound_asset(
        &self,
        hash: &str,
        extension: &str,
    ) -> SyncResult<Option<Vec<u8>>> {
        let (_handle, sftp) = self.connect().await?;
        let path = self
            .remote_path(&format!("assets/sounds/{hash}.{extension}"));
        match sftp.open(&path).await {
            Ok(mut file) => {
                let mut out = Vec::new();
                file.read_to_end(&mut out).await.map_err(|err| {
                    SyncError::network(format!("read sound asset: {err}"))
                })?;
                Ok(Some(out))
            }
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(SyncError::network(format!(
                "open sound asset: {err}"
            ))),
        }
    }
}

/// Write bytes to `path`. Creates / truncates / writes / closes.
async fn write_file(
    sftp: &SftpSession,
    path: &str,
    bytes: &[u8],
) -> SyncResult<()> {
    let mut file = sftp
        .open_with_flags(
            path,
            OpenFlags::CREATE | OpenFlags::WRITE | OpenFlags::TRUNCATE,
        )
        .await
        .map_err(|err| {
            SyncError::network(format!("create {path}: {err}"))
        })?;
    file.write_all(bytes).await.map_err(|err| {
        SyncError::network(format!("write {path}: {err}"))
    })?;
    file.shutdown().await.map_err(|err| {
        SyncError::network(format!("close {path}: {err}"))
    })?;
    Ok(())
}

/// Atomic write via temp + rename. Used for meta.json + snapshot.json
/// so a crash mid-write can't leave a corrupt control file.
async fn atomic_write(
    sftp: &SftpSession,
    path: &str,
    bytes: &[u8],
) -> SyncResult<()> {
    let tmp = format!("{path}.tmp");
    write_file(sftp, &tmp, bytes).await?;
    // POSIX rename is atomic on the same filesystem. SFTP's
    // `rename` translates straight to the server's syscall.
    // Some servers reject rename-over-existing; remove + rename
    // is the portable fallback. Try plain rename first.
    if let Err(err) = sftp.rename(&tmp, path).await {
        debug!(?err, "rename failed; falling back to remove + rename");
        let _ = sftp.remove_file(path).await;
        sftp.rename(&tmp, path).await.map_err(|err| {
            SyncError::network(format!("rename {tmp} → {path}: {err}"))
        })?;
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

    #[test]
    fn remote_path_joins_relative_segments() {
        let a = SftpSyncAdapter::new(
            "h",
            22,
            "u",
            "p",
            PathBuf::from("/home/alice/aperio"),
        );
        assert_eq!(
            a.remote_path("meta.json"),
            "/home/alice/aperio/meta.json",
        );
        assert_eq!(
            a.remote_path("log/2026-05-01T08-00-00Z_dev-a.jsonl"),
            "/home/alice/aperio/log/2026-05-01T08-00-00Z_dev-a.jsonl",
        );
    }

    #[test]
    fn remote_path_trims_redundant_slashes() {
        let a = SftpSyncAdapter::new(
            "h",
            22,
            "u",
            "p",
            PathBuf::from("/home/alice/aperio/"),
        );
        assert_eq!(
            a.remote_path("/meta.json"),
            "/home/alice/aperio/meta.json",
        );
        // Empty relative returns the base.
        assert_eq!(a.remote_path(""), "/home/alice/aperio");
    }

    #[test]
    fn remote_path_normalises_windows_backslashes_in_base() {
        // On Windows the PathBuf may carry backslashes; SFTP
        // demands forward slashes server-side.
        let a = SftpSyncAdapter::new(
            "h",
            22,
            "u",
            "p",
            PathBuf::from("\\home\\alice\\aperio"),
        );
        assert_eq!(a.remote_path("meta.json"), "/home/alice/aperio/meta.json");
    }
}
