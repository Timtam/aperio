//! FTPS / FTP `SyncAdapter` implementation (DESIGN.md §19.6).
//!
//! Pure-Rust client built on [`suppaftp`] + [`rustls`], so the
//! adapter compiles cleanly for Mobile targets (no OpenSSL /
//! SChannel system deps). The DESIGN.md adapter table calls for
//! "FTP über TLS" with username/password auth as the only
//! mode — both encrypted variants (explicit/implicit) are
//! implemented; **plaintext FTP is supported as an opt-in for
//! legacy LAN scenarios** (private networks, isolated VLANs
//! where TLS is genuinely impossible). The settings panel
//! gates the plain mode behind an explicit warning so users
//! don't pick it by accident.
//!
//! Maps the [`SyncAdapter`] trait onto FTP commands:
//!
//! | Trait call           | FTP command(s)                              |
//! |----------------------|---------------------------------------------|
//! | `test_connection`    | Connect + login + `NOOP` + `MKD` (lazy)     |
//! | `fetch_meta`         | `RETR <base>/meta.json`                     |
//! | `push_meta`          | `STOR <tmp>` + `RNFR`/`RNTO` to meta.json   |
//! | `fetch_new_logs`     | `NLST <base>/log/` + per-file `RETR`        |
//! | `push_log`           | `STOR <base>/log/<filename>`                |
//! | `fetch/push_snap`    | `RETR` / `STOR`+`RNTO` over snapshot.json   |
//! | `delete_log`         | `DELE`                                       |
//! | sound asset CRUD     | `<base>/assets/sounds/<hash>.<ext>`         |
//!
//! ## Three modes
//!
//! - **Explicit FTPS** (default, port 21): connect plaintext,
//!   then issue `AUTH TLS` to upgrade the control channel. Data
//!   channels reuse the negotiated TLS state. The more
//!   compatible mode — most servers either support it or refuse
//!   the upgrade cleanly.
//!
//! - **Implicit FTPS** (port 990 by convention): TLS handshake
//!   happens before the FTP greeting. Slightly faster (one
//!   fewer round-trip) but rarer in the wild.
//!
//! - **Plain FTP** (no TLS, port 21): credentials + payload
//!   traverse the network unencrypted. Provided for legacy LAN
//!   scenarios — an isolated VLAN with an FTP server that
//!   genuinely can't do TLS, or testing setups where the cost
//!   of a self-signed cert outweighs the lack of confidentiality.
//!   The frontend gates this mode behind a visible warning;
//!   never use it across an untrusted network.
//!
//! ## Async strategy: sync API + `spawn_blocking`
//!
//! `suppaftp`'s async surface is built on `async-std`; mixing
//! that with our tokio runtime would mean either bridging
//! reactors (fragile) or running two runtimes (heavy). The
//! cleaner path: use suppaftp's sync API and offload each call
//! onto tokio's blocking pool. Per-operation connects take
//! ~150-300 ms (TCP + TLS handshake + FTP greeting + login);
//! the default sync interval is 5 minutes so the blocking-pool
//! occupancy is negligible.
//!
//! ## Atomic writes
//!
//! `meta.json` + `snapshot.json` use `STOR`-tmp + `RNFR`/`RNTO`
//! so a crash mid-write can't leave the control file corrupt at
//! the canonical path. Most FTP servers implement RNTO as a
//! single inode-rename, which is atomic in practice even though
//! RFC 959 doesn't promise it. Log files don't need atomic
//! writes — their filenames embed device id + timestamp, so a
//! partial upload is retried verbatim by the scheduler.
//!
//! ## Passive mode
//!
//! Passive mode is mandatory (`PASV` / `EPSV`). `suppaftp`
//! defaults to passive for every data-mode command. Active
//! mode would require the client to accept an incoming TCP
//! connection from the server, which fails behind NAT — and
//! §19.6 explicitly targets the home-NAS scenario where the
//! client is always behind NAT.
//!
//! ## What this crate does NOT do
//!
//! - **Connection pooling.** Per-operation connect.
//! - **MLSD / MLST.** We parse `NLST` output (one filename per
//!   line) — simpler + universally supported. Aperio data
//!   filenames are timestamp-prefixed so client-side parsing is
//!   deterministic without server-supplied metadata.
//! - **Self-signed cert trust.** TLS validation uses Mozilla's
//!   webpki-roots; self-signed certs fail at handshake. Users
//!   on private NAS deployments should install their CA into
//!   the system trust store at the OS level (out of scope for
//!   v1).

use std::sync::Arc;

use async_trait::async_trait;
use rustls::{ClientConfig, RootCertStore};
use serde::{Deserialize, Serialize};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
    SyncError, SyncResult,
};
use tracing::{debug, warn};

use suppaftp::types::FileType;
use suppaftp::FtpError;
use suppaftp::FtpResult;
use suppaftp::FtpStream;
use suppaftp::RustlsConnector;
use suppaftp::RustlsFtpStream;
use suppaftp::Status;

// ─────────────────────────────────────────────────────────────────
// Config types
// ─────────────────────────────────────────────────────────────────

/// FTPS mode picker — the three ways FTP can sit on the wire:
/// two encrypted variants and a legacy plain-text fallback for
/// the isolated-LAN scenarios where TLS is genuinely impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FtpsMode {
    /// AUTH TLS upgrade. Connect plaintext to the FTP port
    /// (typically 21), issue `AUTH TLS`, then everything from
    /// the login onwards is encrypted. The default — most
    /// servers support it.
    Explicit,
    /// TLS handshake before the FTP greeting (typically port
    /// 990). Slightly faster but rarer.
    Implicit,
    /// Plaintext FTP (no TLS). Credentials and payload are
    /// visible to anyone on the wire. Provided for legacy
    /// LAN-only setups where TLS isn't an option — the
    /// frontend warns explicitly before this mode can be
    /// selected. NEVER use across an untrusted network.
    Plain,
}

impl FtpsMode {
    /// Default port for this mode: 21 for explicit + plain
    /// (they share port 21 on the wire), 990 for implicit.
    /// Used by the settings dialog to suggest a sensible port
    /// when the user picks the mode.
    pub fn default_port(self) -> u16 {
        match self {
            Self::Explicit | Self::Plain => 21,
            Self::Implicit => 990,
        }
    }

    /// Whether this mode runs the FTP control + data channels
    /// over TLS. `false` for `Plain` only.
    pub fn is_encrypted(self) -> bool {
        !matches!(self, Self::Plain)
    }
}

/// Internal stream wrapper — picks the right suppaftp stream
/// type at runtime so each [`SyncAdapter`] trait method can
/// dispatch uniformly. Without this we'd have to duplicate
/// every method body across the two FTP modes (TLS vs. plain
/// have different generic instantiations of
/// `suppaftp::ImplFtpStream<T>`).
enum SessionStream {
    Tls(RustlsFtpStream),
    Plain(FtpStream),
}

impl SessionStream {
    fn login(&mut self, user: &str, password: &str) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.login(user, password),
            Self::Plain(s) => s.login(user, password),
        }
    }

    fn transfer_type(&mut self, ty: FileType) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.transfer_type(ty),
            Self::Plain(s) => s.transfer_type(ty),
        }
    }

    fn noop(&mut self) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.noop(),
            Self::Plain(s) => s.noop(),
        }
    }

    fn mkdir(&mut self, path: &str) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.mkdir(path),
            Self::Plain(s) => s.mkdir(path),
        }
    }

    fn retr_as_buffer(
        &mut self,
        path: &str,
    ) -> FtpResult<std::io::Cursor<Vec<u8>>> {
        match self {
            Self::Tls(s) => s.retr_as_buffer(path),
            Self::Plain(s) => s.retr_as_buffer(path),
        }
    }

    fn put_file(
        &mut self,
        path: &str,
        r: &mut std::io::Cursor<&[u8]>,
    ) -> FtpResult<u64> {
        match self {
            Self::Tls(s) => s.put_file(path, r),
            Self::Plain(s) => s.put_file(path, r),
        }
    }

    fn nlst(&mut self, path: Option<&str>) -> FtpResult<Vec<String>> {
        match self {
            Self::Tls(s) => s.nlst(path),
            Self::Plain(s) => s.nlst(path),
        }
    }

    fn rm(&mut self, path: &str) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.rm(path),
            Self::Plain(s) => s.rm(path),
        }
    }

    fn rename(&mut self, from: &str, to: &str) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.rename(from, to),
            Self::Plain(s) => s.rename(from, to),
        }
    }

    fn quit(&mut self) -> FtpResult<()> {
        match self {
            Self::Tls(s) => s.quit(),
            Self::Plain(s) => s.quit(),
        }
    }
}

/// FTPS `SyncAdapter`. Cheap to clone — owns small config
/// strings + a shared TLS config that's built once per adapter
/// and reused across every connect.
#[derive(Debug, Clone)]
pub struct FtpsSyncAdapter {
    host: String,
    port: u16,
    user: String,
    password: String,
    base_path: String,
    mode: FtpsMode,
    tls_config: Arc<ClientConfig>,
}

impl FtpsSyncAdapter {
    /// Build an adapter against a host:port + base directory.
    /// The base path is the remote directory that holds
    /// `log/`, `snapshot.json`, etc. — typically something
    /// like `/aperio` or `/srv/sync/aperio`. Leading and
    /// trailing slashes are normalised so the caller doesn't
    /// have to worry about either form.
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        password: impl Into<String>,
        base_path: impl Into<String>,
        mode: FtpsMode,
    ) -> Self {
        let base_raw = base_path.into();
        let base_path = normalise_base(&base_raw);
        Self {
            host: host.into(),
            port,
            user: user.into(),
            password: password.into(),
            base_path,
            mode,
            tls_config: Arc::new(default_tls_config()),
        }
    }

    /// Join `relative` onto the base path. `relative` MUST NOT
    /// start with a slash; `base_path` is guaranteed to be
    /// either empty (server root) or start with `/` without a
    /// trailing slash.
    fn remote_path(&self, relative: &str) -> String {
        remote_path_for(&self.base_path, relative)
    }

    /// Run a closure on the blocking pool with a freshly-opened
    /// FTP connection. The closure receives `&mut SessionStream`
    /// after login + binary-mode setup, regardless of whether
    /// the underlying transport is TLS or plain — the wrapper
    /// dispatches per call. Returns whatever the closure
    /// returns; any FTP error short-circuits as `SyncError`.
    ///
    /// Per-operation connect — see the module-level doc for
    /// the rationale.
    async fn with_session<F, T>(&self, work: F) -> SyncResult<T>
    where
        F: FnOnce(&mut SessionStream) -> SyncResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let host = self.host.clone();
        let port = self.port;
        let user = self.user.clone();
        let password = self.password.clone();
        let mode = self.mode;
        let tls = self.tls_config.clone();

        tokio::task::spawn_blocking(move || {
            let addr = format!("{host}:{port}");
            let mut stream = match mode {
                FtpsMode::Explicit => {
                    let connector = RustlsConnector::from(tls);
                    let plain = RustlsFtpStream::connect(&addr).map_err(
                        |err| {
                            SyncError::network(format!(
                                "connect {addr}: {err}"
                            ))
                        },
                    )?;
                    let secured =
                        plain.into_secure(connector, &host).map_err(|err| {
                            SyncError::network(format!(
                                "AUTH TLS to {host}: {err}"
                            ))
                        })?;
                    SessionStream::Tls(secured)
                }
                FtpsMode::Implicit => {
                    let connector = RustlsConnector::from(tls);
                    let secured = RustlsFtpStream::connect_secure_implicit(
                        &addr, connector, &host,
                    )
                    .map_err(|err| {
                        SyncError::network(format!(
                            "implicit TLS connect {addr}: {err}"
                        ))
                    })?;
                    SessionStream::Tls(secured)
                }
                FtpsMode::Plain => {
                    let plain = FtpStream::connect(&addr).map_err(|err| {
                        SyncError::network(format!(
                            "plain connect {addr}: {err}"
                        ))
                    })?;
                    SessionStream::Plain(plain)
                }
            };

            stream.login(&user, &password).map_err(ftp_to_sync_auth)?;
            stream.transfer_type(FileType::Binary).map_err(|err| {
                SyncError::protocol(format!("TYPE I: {err}"))
            })?;
            let result = work(&mut stream);
            let _ = stream.quit();
            result
        })
        .await
        .map_err(|err| {
            SyncError::internal(format!("blocking task join: {err}"))
        })?
    }
}

#[async_trait]
impl SyncAdapter for FtpsSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        let base = self.base_path.clone();
        let base_for_paths = self.base_path.clone();
        self.with_session(move |stream| {
            stream.noop().map_err(|err| {
                SyncError::network(format!("NOOP: {err}"))
            })?;
            // Best-effort mkdir on the base + sub-collections so
            // first-push works on a fresh server. AlreadyExists
            // fast-paths to Ok.
            if !base.is_empty() {
                ensure_dir(stream, &base)?;
            }
            ensure_dir(stream, &remote_path_for(&base_for_paths, "log"))?;
            ensure_dir(stream, &remote_path_for(&base_for_paths, "assets"))?;
            ensure_dir(
                stream,
                &remote_path_for(&base_for_paths, "assets/sounds"),
            )?;
            Ok(())
        })
        .await
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let path = self.remote_path("meta.json");
        let bytes = self
            .with_session(move |stream| match stream.retr_as_buffer(&path)
            {
                Ok(buf) => Ok(Some(buf.into_inner())),
                Err(err) if is_not_found(&err) => Ok(None),
                Err(err) => Err(SyncError::network(format!(
                    "RETR meta.json: {err}"
                ))),
            })
            .await?;
        match bytes {
            Some(b) => Ok(Some(MetaJson::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let base = self.base_path.clone();
        let path = self.remote_path("meta.json");
        let bytes = meta.to_bytes()?;
        self.with_session(move |stream| {
            if !base.is_empty() {
                ensure_dir(stream, &base)?;
            }
            atomic_write(stream, &path, &bytes)
        })
        .await
    }

    async fn fetch_new_logs(
        &self,
        since: &DeviceCursor,
    ) -> SyncResult<Vec<LogFile>> {
        let log_dir = self.remote_path("log");
        let cursor_ts = since.last_seen_log;
        self.with_session(move |stream| {
            // NLST returns one filename per line. Some servers
            // include the directory prefix; some don't. Strip the
            // basename before handing to LogFileName::from_filename,
            // which validates the timestamp + device-id shape.
            let entries = match stream.nlst(Some(&log_dir)) {
                Ok(e) => e,
                Err(err) if is_not_found(&err) => return Ok(Vec::new()),
                Err(err) => {
                    return Err(SyncError::network(format!(
                        "NLST log/: {err}"
                    )));
                }
            };

            let mut wanted: Vec<LogFileName> = Vec::new();
            for raw in entries {
                let name = raw.rsplit('/').next().unwrap_or(&raw);
                let parsed = match LogFileName::from_filename(name) {
                    Ok(p) => p,
                    Err(_) => {
                        debug!(name = %name, "skipping non-log entry in NLST");
                        continue;
                    }
                };
                if parsed.timestamp > cursor_ts {
                    wanted.push(parsed);
                }
            }

            let mut out = Vec::with_capacity(wanted.len());
            for parsed in wanted {
                let path = format!("{}/{}", log_dir, parsed.to_filename());
                match stream.retr_as_buffer(&path) {
                    Ok(buf) => out.push(LogFile {
                        name: parsed,
                        bytes: buf.into_inner(),
                    }),
                    Err(err) if is_not_found(&err) => {
                        // Compactor raced us between NLST + RETR;
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
                            "RETR log file failed; skipping",
                        );
                    }
                }
            }
            Ok(out)
        })
        .await
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let log_dir = self.remote_path("log");
        let path = self
            .remote_path(&format!("log/{}", log.name.to_filename()));
        let bytes = log.bytes.clone();
        self.with_session(move |stream| {
            ensure_dir(stream, &log_dir)?;
            write_file(stream, &path, &bytes)
        })
        .await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let path = self.remote_path("snapshot.json");
        let bytes = self
            .with_session(move |stream| match stream.retr_as_buffer(&path)
            {
                Ok(buf) => Ok(Some(buf.into_inner())),
                Err(err) if is_not_found(&err) => Ok(None),
                Err(err) => Err(SyncError::network(format!(
                    "RETR snapshot.json: {err}"
                ))),
            })
            .await?;
        match bytes {
            Some(b) => Ok(Some(Snapshot::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let base = self.base_path.clone();
        let path = self.remote_path("snapshot.json");
        let bytes = snapshot.to_bytes()?;
        self.with_session(move |stream| {
            if !base.is_empty() {
                ensure_dir(stream, &base)?;
            }
            atomic_write(stream, &path, &bytes)
        })
        .await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let path = self.remote_path(&format!("log/{}", name.to_filename()));
        self.with_session(move |stream| {
            match stream.rm(&path) {
                Ok(()) => Ok(()),
                // Not-found is success: the goal is "make sure
                // it's gone", and absent already satisfies that.
                Err(err) if is_not_found(&err) => Ok(()),
                Err(err) => Err(SyncError::network(format!(
                    "DELE {path}: {err}"
                ))),
            }
        })
        .await
    }

    async fn push_sound_asset(
        &self,
        hash: &str,
        extension: &str,
        bytes: &[u8],
    ) -> SyncResult<()> {
        let assets_dir = self.remote_path("assets");
        let sounds_dir = self.remote_path("assets/sounds");
        let path = self
            .remote_path(&format!("assets/sounds/{hash}.{extension}"));
        let payload = bytes.to_vec();
        self.with_session(move |stream| {
            ensure_dir(stream, &assets_dir)?;
            ensure_dir(stream, &sounds_dir)?;
            write_file(stream, &path, &payload)
        })
        .await
    }

    async fn fetch_sound_asset(
        &self,
        hash: &str,
        extension: &str,
    ) -> SyncResult<Option<Vec<u8>>> {
        let path = self
            .remote_path(&format!("assets/sounds/{hash}.{extension}"));
        self.with_session(move |stream| match stream.retr_as_buffer(&path) {
            Ok(buf) => Ok(Some(buf.into_inner())),
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(SyncError::network(format!(
                "RETR sound asset: {err}"
            ))),
        })
        .await
    }
}

// ─────────────────────────────────────────────────────────────────
// Sync helpers — invoked from inside the blocking closure
// ─────────────────────────────────────────────────────────────────

/// Ensure `dir` exists on the remote. Idempotent: an "already
/// exists" response is folded into success. Sync because it
/// runs inside `spawn_blocking`.
fn ensure_dir(stream: &mut SessionStream, dir: &str) -> SyncResult<()> {
    match stream.mkdir(dir) {
        Ok(()) => Ok(()),
        Err(err) if is_already_exists(&err) => Ok(()),
        Err(err) => Err(SyncError::network(format!("MKD {dir}: {err}"))),
    }
}

/// `STOR` the given bytes to `path`.
fn write_file(
    stream: &mut SessionStream,
    path: &str,
    bytes: &[u8],
) -> SyncResult<()> {
    let mut cursor = std::io::Cursor::new(bytes);
    stream.put_file(path, &mut cursor).map_err(|err| {
        SyncError::network(format!("STOR {path}: {err}"))
    })?;
    Ok(())
}

/// `STOR` to a temp path then `RNFR`/`RNTO` over the canonical
/// name. Most FTP servers implement RNTO as an atomic inode
/// rename, which is the closest FTP gets to the local-FS
/// atomic-replace pattern.
fn atomic_write(
    stream: &mut SessionStream,
    path: &str,
    bytes: &[u8],
) -> SyncResult<()> {
    let tmp = format!("{path}.tmp");
    write_file(stream, &tmp, bytes)?;
    // Best-effort: drop a stale destination if it exists.
    // RFC 959 allows RNTO over an existing file but many
    // servers refuse it; explicit cleanup keeps the happy
    // path predictable.
    let _ = stream.rm(path);
    // `rename` is generic over `S: AsRef<str>` with both args
    // sharing the same type parameter — pin both to `&str` so
    // the &String/&str mismatch doesn't bite us.
    let from: &str = &tmp;
    stream.rename(from, path).map_err(|err| {
        SyncError::network(format!("RNFR/RNTO {tmp}->{path}: {err}"))
    })?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────
// Pure helpers
// ─────────────────────────────────────────────────────────────────

/// Join `relative` onto the base path. `relative` MUST NOT
/// start with a slash; `base` is guaranteed to be either
/// empty (server root) or start with `/` without a trailing
/// slash.
fn remote_path_for(base: &str, relative: &str) -> String {
    if base.is_empty() {
        if relative.is_empty() {
            "/".to_string()
        } else {
            format!("/{relative}")
        }
    } else if relative.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{relative}")
    }
}

/// Normalise a user-supplied base path into the canonical form
/// the adapter expects: empty (server root) or `/foo/bar`
/// (leading slash, no trailing slash).
fn normalise_base(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "/" {
        return String::new();
    }
    let with_lead = if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    };
    let trimmed_trail = with_lead.trim_end_matches('/').to_string();
    if trimmed_trail.is_empty() {
        String::new()
    } else {
        trimmed_trail
    }
}

/// Default rustls config: Mozilla's webpki-roots as trust
/// store, no client auth, default cipher suites. Shared across
/// the adapter's lifetime since the config is immutable.
fn default_tls_config() -> ClientConfig {
    let mut root_store = RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth()
}

/// FTP error → `SyncError::Auth` for the credentials-rejected
/// case (530 Not logged in, etc.). Everything else surfaces as
/// `SyncError::Network` since the connection itself worked.
fn ftp_to_sync_auth(err: FtpError) -> SyncError {
    if is_auth_failure(&err) {
        SyncError::auth(format!("FTP login: {err}"))
    } else {
        SyncError::network(format!("FTP login: {err}"))
    }
}

/// 550 "file unavailable" — the canonical "no such file / no
/// such directory" response. Folded into `Ok(None)` semantics
/// at the upper layer.
fn is_not_found(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response)
            if response.status == Status::FileUnavailable
    )
}

/// "Directory already exists" responses to MKD. Different
/// servers send slightly different codes; matching the body
/// for the substring "exist" covers both 521 and 550 variants.
/// `response.body` is the raw bytes of the FTP server's
/// reply.
fn is_already_exists(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response)
            if String::from_utf8_lossy(&response.body)
                .to_ascii_lowercase()
                .contains("exist")
    )
}

/// 530 Not logged in / auth-class failures. Distinguishes
/// credential rejection (user-actionable: re-type password)
/// from generic network failures.
fn is_auth_failure(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response)
            if response.status == Status::NotLoggedIn
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_base_handles_common_shapes() {
        assert_eq!(normalise_base(""), "");
        assert_eq!(normalise_base("/"), "");
        assert_eq!(normalise_base("aperio"), "/aperio");
        assert_eq!(normalise_base("/aperio"), "/aperio");
        assert_eq!(normalise_base("/aperio/"), "/aperio");
        assert_eq!(normalise_base("aperio/"), "/aperio");
        assert_eq!(normalise_base("/srv/aperio/"), "/srv/aperio");
        // Trailing whitespace from a sloppy paste stays trimmed.
        assert_eq!(normalise_base("  /aperio  "), "/aperio");
    }

    #[test]
    fn remote_path_joins_against_normalised_base() {
        assert_eq!(remote_path_for("/aperio", ""), "/aperio");
        assert_eq!(
            remote_path_for("/aperio", "meta.json"),
            "/aperio/meta.json",
        );
        assert_eq!(
            remote_path_for(
                "/aperio",
                "log/2026-01-01T00:00:00Z_dev-a.jsonl",
            ),
            "/aperio/log/2026-01-01T00:00:00Z_dev-a.jsonl",
        );

        assert_eq!(remote_path_for("", ""), "/");
        assert_eq!(remote_path_for("", "meta.json"), "/meta.json");
    }

    #[test]
    fn ftps_mode_default_ports() {
        assert_eq!(FtpsMode::Explicit.default_port(), 21);
        assert_eq!(FtpsMode::Implicit.default_port(), 990);
        // Plain shares the explicit-FTPS port — server-side
        // they're the same listener (the AUTH TLS command is
        // what flips encryption on).
        assert_eq!(FtpsMode::Plain.default_port(), 21);
    }

    #[test]
    fn ftps_mode_is_encrypted_flag() {
        assert!(FtpsMode::Explicit.is_encrypted());
        assert!(FtpsMode::Implicit.is_encrypted());
        assert!(!FtpsMode::Plain.is_encrypted());
    }
}
