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
//! | `fetch_new_logs`     | `MLSD` or `NLST`+`SIZE`, then per-file `RETR` |
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
//! onto tokio's blocking pool. Opening a control connection
//! takes ~150-300 ms (TCP + TLS handshake + FTP greeting +
//! login); the reuse layer below makes a sync round pay that
//! preamble once instead of once per trait call, and the
//! default sync interval is 5 minutes so the blocking-pool
//! occupancy is negligible either way.
//!
//! ## Control-connection reuse
//!
//! One logged-in control connection is parked on the adapter
//! (shared across clones) and reused while it is still warm — a
//! sync round fires several trait calls back-to-back, and each
//! used to pay the full connect preamble. The parked session
//! expires after a short idle TTL (~3 s, mirroring the WebDAV
//! adapter's `pool_idle_timeout` rationale): NAT gateways and
//! home routers reap idle FTP control sockets aggressively, so
//! a socket idle longer than the sub-second within-round gaps
//! is never handed out again. If a reused socket turns out dead
//! mid-call anyway, the call is retried ONCE on a fresh
//! connection — every trait operation is idempotent (pure
//! reads, overwrite-`STOR`s, `DELE` folds not-found), so the
//! duplicate attempt is safe.
//!
//! ## Atomic writes
//!
//! `meta.json` + `snapshot.json` use `STOR`-tmp + `RNFR`/`RNTO`
//! so a crash mid-write can't leave the control file corrupt at
//! the canonical path. The rename targets the destination
//! directly — RFC 959 permits `RNTO` over an existing file — so
//! the canonical path never has a missing-file window (a peer
//! reading `meta.json` mid-write must never see `Ok(None)` =
//! "fresh dataset"). Only when a server refuses the overwrite
//! (550/553) do we `DELE` and retry the rename once. Most FTP
//! servers implement RNTO as a single inode-rename, which is
//! atomic in practice even though RFC 959 doesn't promise it.
//! Log files don't need atomic writes — their filenames embed
//! device id + timestamp, so a partial upload is retried
//! verbatim by the scheduler.
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
//! - **`LIST` parsing.** Directory listings use `MLSD` when the
//!   server advertises RFC 3659 (machine-readable, carries the
//!   byte sizes the growth-refetch check needs) and fall back
//!   to `NLST` plus targeted `SIZE` probes otherwise. The
//!   human-readable `LIST` output differs per server and is
//!   never parsed.
//! - **Cross-round pooling.** The parked control connection
//!   expires after seconds; every sync round still opens (one)
//!   fresh connection.
//! - **Self-signed cert trust.** TLS validation uses Mozilla's
//!   webpki-roots; self-signed certs fail at handshake. Users
//!   on private NAS deployments should install their CA into
//!   the system trust store at the OS level (out of scope for
//!   v1).

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rustls::{ClientConfig, RootCertStore};
use serde::{Deserialize, Serialize};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter, SyncError, SyncResult,
};
use tracing::{debug, warn};

use suppaftp::types::Features;
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

    fn retr_as_buffer(&mut self, path: &str) -> FtpResult<std::io::Cursor<Vec<u8>>> {
        match self {
            Self::Tls(s) => s.retr_as_buffer(path),
            Self::Plain(s) => s.retr_as_buffer(path),
        }
    }

    fn put_file(&mut self, path: &str, r: &mut std::io::Cursor<&[u8]>) -> FtpResult<u64> {
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

    fn mlsd(&mut self, path: Option<&str>) -> FtpResult<Vec<String>> {
        match self {
            Self::Tls(s) => s.mlsd(path),
            Self::Plain(s) => s.mlsd(path),
        }
    }

    fn feat(&mut self) -> FtpResult<Features> {
        match self {
            Self::Tls(s) => s.feat(),
            Self::Plain(s) => s.feat(),
        }
    }

    fn size(&mut self, path: &str) -> FtpResult<usize> {
        match self {
            Self::Tls(s) => s.size(path),
            Self::Plain(s) => s.size(path),
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

/// The one parked control connection + when it was parked. Kept
/// behind an `Arc` on the adapter so clones share it (a sync
/// round clones the adapter per call but must reuse one session).
type SessionSlot = Mutex<Option<(SessionStream, Instant)>>;

/// How long a parked control connection may sit idle before we
/// refuse to reuse it. Mirrors the WebDAV adapter's
/// `pool_idle_timeout(3)` rationale: NAT gateways / home routers
/// / conservative servers reap idle control sockets, so never
/// hand out a socket the far end may have dropped. Within-round
/// gaps are sub-second and rounds are minutes apart, so 3 s
/// cleanly separates "same round" from "next round".
const SESSION_IDLE_TTL: Duration = Duration::from_secs(3);

/// FTPS `SyncAdapter`. Cheap to clone — owns small config
/// strings, a shared TLS config that's built once per adapter
/// and reused across every connect, plus the shared session /
/// capability caches (all `Arc`s, so clones observe one state).
#[derive(Clone)]
pub struct FtpsSyncAdapter {
    host: String,
    port: u16,
    user: String,
    password: String,
    base_path: String,
    mode: FtpsMode,
    tls_config: Arc<ClientConfig>,
    /// Parked control connection for the short-TTL reuse layer.
    session: Arc<SessionSlot>,
    /// Directories `MKD`'d this session (the WebDAV adapter's
    /// `ensure_collection` cache, ported). Remote directories
    /// persist server-side, so one `MKD` per path per session is
    /// enough; cleared when a reused connection fails (server
    /// state suspect) and by `test_connection` (explicit probe).
    ensured: Arc<Mutex<HashSet<String>>>,
    /// FEAT probe result: whether the server advertises RFC 3659
    /// `MLSD`. `None` until the first `fetch_new_logs` asks; the
    /// verdict is stable for a server, so one probe per adapter
    /// lifetime suffices.
    mlsd_supported: Arc<Mutex<Option<bool>>>,
}

impl std::fmt::Debug for FtpsSyncAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Manual impl: `SessionStream` has no `Debug`, and a derive
        // would print the password into logs.
        f.debug_struct("FtpsSyncAdapter")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("base_path", &self.base_path)
            .field("mode", &self.mode)
            .finish_non_exhaustive()
    }
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
            session: Arc::new(Mutex::new(None)),
            ensured: Arc::new(Mutex::new(HashSet::new())),
            mlsd_supported: Arc::new(Mutex::new(None)),
        }
    }

    /// Join `relative` onto the base path. `relative` MUST NOT
    /// start with a slash; `base_path` is guaranteed to be
    /// either empty (server root) or start with `/` without a
    /// trailing slash.
    fn remote_path(&self, relative: &str) -> String {
        remote_path_for(&self.base_path, relative)
    }

    /// Run a closure on the blocking pool with a logged-in
    /// control connection. The closure receives `&mut
    /// SessionStream` after login + binary-mode setup,
    /// regardless of whether the underlying transport is TLS or
    /// plain — the wrapper dispatches per call.
    ///
    /// The connection comes from the shared parked slot when it
    /// is still warm (see the module doc's reuse section) and is
    /// parked back on success. When a REUSED connection fails,
    /// the closure runs ONCE more on a fresh connection — so the
    /// closure is `FnMut` and every call site must stay
    /// idempotent, which the `SyncAdapter` contract already
    /// guarantees.
    async fn with_session<F, T>(&self, work: F) -> SyncResult<T>
    where
        F: FnMut(&mut SessionStream) -> SyncResult<T> + Send + 'static,
        T: Send + 'static,
    {
        let host = self.host.clone();
        let port = self.port;
        let user = self.user.clone();
        let password = self.password.clone();
        let mode = self.mode;
        let tls = self.tls_config.clone();
        let session = self.session.clone();
        let ensured = self.ensured.clone();

        tokio::task::spawn_blocking(move || {
            run_with_reuse(
                &session,
                SESSION_IDLE_TTL,
                || open_session(&host, port, &user, &password, mode, tls.clone()),
                |mut stream| {
                    // Polite best-effort QUIT on a session we are
                    // dropping after a failure — if the socket is
                    // dead this errors instantly and the drop's
                    // FIN does the rest.
                    let _ = stream.quit();
                },
                || {
                    // A failure on a reused connection means the
                    // server state is suspect (reset? dataset
                    // wiped?) — drop the ensured-directory cache
                    // so the retry re-creates what's missing
                    // instead of assuming it's intact.
                    ensured.lock().expect("ensured mutex poison").clear();
                },
                work,
            )
        })
        .await
        .map_err(|err| SyncError::internal(format!("blocking task join: {err}")))?
    }
}

#[async_trait]
impl SyncAdapter for FtpsSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        let base = self.base_path.clone();
        let ensured = self.ensured.clone();
        self.with_session(move |stream| {
            stream
                .noop()
                .map_err(|err| SyncError::network(format!("NOOP: {err}")))?;
            // The Connect button is an explicit probe — drop the
            // ensured cache first so a wiped server gets its tree
            // re-created rather than assumed intact.
            ensured.lock().expect("ensured mutex poison").clear();
            // Best-effort mkdir on the base + sub-collections so
            // first-push works on a fresh server. AlreadyExists
            // fast-paths to Ok. Seeds the ensured cache, so later
            // pushes skip their own MKDs.
            if !base.is_empty() {
                ensure_dir_cached(stream, &ensured, &base)?;
            }
            ensure_dir_cached(stream, &ensured, &remote_path_for(&base, "log"))?;
            ensure_dir_cached(stream, &ensured, &remote_path_for(&base, "assets"))?;
            ensure_dir_cached(stream, &ensured, &remote_path_for(&base, "assets/sounds"))?;
            Ok(())
        })
        .await
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let path = self.remote_path("meta.json");
        let bytes = self
            .with_session(move |stream| match stream.retr_as_buffer(&path) {
                Ok(buf) => Ok(Some(buf.into_inner())),
                Err(err) if is_not_found(&err) => Ok(None),
                Err(err) => Err(SyncError::network(format!("RETR meta.json: {err}"))),
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
        let ensured = self.ensured.clone();
        self.with_session(move |stream| {
            if !base.is_empty() {
                ensure_dir_cached(stream, &ensured, &base)?;
            }
            atomic_write(stream, &path, &bytes)
        })
        .await
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        let log_dir = self.remote_path("log");
        let cursor = since.clone();
        let mlsd_cache = self.mlsd_supported.clone();
        self.with_session(move |stream| {
            // Listing strategy: MLSD when the server advertises
            // RFC 3659 — machine-readable AND carries byte sizes,
            // which feed the cursor's growth-refetch check (a
            // peer's live session file that gained appended events
            // sits at/below the cursor and would otherwise be
            // missed until — and permanently after — the peer
            // rotates it). Servers without MLSD fall back to NLST
            // plus targeted SIZE probes.
            //
            // Sizes flow into `wants_sized` RAW: under E2E the
            // cursor's known_lengths were already translated into
            // the ciphertext domain by the EncryptingAdapter, so
            // the remote's byte counts are exactly the domain the
            // cursor expects — never adjust them here.
            let entries: Vec<(String, Option<u64>)> = if mlsd_advertised(stream, &mlsd_cache) {
                match stream.mlsd(Some(&log_dir)) {
                    Ok(lines) => {
                        let entries: Vec<_> = lines
                            .iter()
                            .map(String::as_str)
                            .filter_map(parse_mlsx_entry)
                            .collect();
                        // Belt-and-braces against server dialects
                        // the parser doesn't understand: a listing
                        // with lines but zero file entries is
                        // either a legitimately empty directory
                        // (cdir/pdir rows only) or every line
                        // failing to parse — and the latter would
                        // otherwise present as a silent, permanent
                        // "no peer logs". Make it diagnosable.
                        if !lines.is_empty() && entries.is_empty() {
                            warn!(
                                dir = %log_dir,
                                raw_lines = lines.len(),
                                "MLSD returned lines but no file entries parsed",
                            );
                        }
                        entries
                    }
                    Err(err) if is_not_found(&err) => return Ok(Vec::new()),
                    Err(err) => {
                        return Err(SyncError::network(format!("MLSD log/: {err}")));
                    }
                }
            } else {
                // NLST returns one filename per line, no sizes.
                // Some servers include the directory prefix; some
                // don't. Strip the basename, then probe SIZE only
                // where the growth check can actually use the
                // answer — names with a recorded applied length
                // (bounded: one live session file per peer device,
                // so 1-3 extra control round trips).
                let raw_names = match stream.nlst(Some(&log_dir)) {
                    Ok(e) => e,
                    Err(err) if is_not_found(&err) => return Ok(Vec::new()),
                    Err(err) => {
                        return Err(SyncError::network(format!("NLST log/: {err}")));
                    }
                };
                let mut entries = Vec::with_capacity(raw_names.len());
                for raw in raw_names {
                    let name = basename(&raw).to_string();
                    let size = match LogFileName::from_filename(&name) {
                        Ok(parsed) if needs_size_probe(&cursor, &parsed, &name) => {
                            let path = format!("{log_dir}/{name}");
                            match stream.size(&path) {
                                Ok(s) => Some(s as u64),
                                // SIZE failing (unsupported command,
                                // file raced away) degrades to "size
                                // unknown": the growth check skips
                                // this round and re-probes the next.
                                Err(err) => {
                                    debug!(
                                        path = %path,
                                        ?err,
                                        "SIZE probe failed; treating length as unknown",
                                    );
                                    None
                                }
                            }
                        }
                        _ => None,
                    };
                    entries.push((name, size));
                }
                entries
            };

            let wanted = select_wanted(&entries, &cursor);

            let mut out = Vec::with_capacity(wanted.len());
            for parsed in wanted {
                let path = format!("{}/{}", log_dir, parsed.to_filename());
                match stream.retr_as_buffer(&path) {
                    Ok(buf) => out.push(LogFile {
                        name: parsed,
                        bytes: buf.into_inner(),
                    }),
                    Err(err) => {
                        match retr_error_disposition(&err, || stream.size(&path)) {
                            RetrDisposition::SkipMissing => {
                                // Compactor raced us between the
                                // listing and the RETR — absence
                                // confirmed by the SIZE probe. Warn
                                // (not debug) so a recurring skip is
                                // diagnosable in the field.
                                warn!(
                                    path = %path,
                                    "log file listed but confirmed gone; skipping",
                                );
                            }
                            RetrDisposition::FailBatch => {
                                return Err(SyncError::network(format!("RETR {path}: {err}")));
                            }
                        }
                    }
                }
            }
            Ok(out)
        })
        .await
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let log_dir = self.remote_path("log");
        let path = self.remote_path(&format!("log/{}", log.name.to_filename()));
        let bytes = log.bytes.clone();
        let ensured = self.ensured.clone();
        self.with_session(move |stream| {
            ensure_dir_cached(stream, &ensured, &log_dir)?;
            write_file(stream, &path, &bytes)
        })
        .await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let path = self.remote_path("snapshot.json");
        let bytes = self
            .with_session(move |stream| match stream.retr_as_buffer(&path) {
                Ok(buf) => Ok(Some(buf.into_inner())),
                Err(err) if is_not_found(&err) => Ok(None),
                Err(err) => Err(SyncError::network(format!("RETR snapshot.json: {err}"))),
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
        let ensured = self.ensured.clone();
        self.with_session(move |stream| {
            if !base.is_empty() {
                ensure_dir_cached(stream, &ensured, &base)?;
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
                Err(err) => Err(SyncError::network(format!("DELE {path}: {err}"))),
            }
        })
        .await
    }

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        let assets_dir = self.remote_path("assets");
        let sounds_dir = self.remote_path("assets/sounds");
        let path = self.remote_path(&format!("assets/sounds/{hash}.{extension}"));
        let payload = bytes.to_vec();
        let ensured = self.ensured.clone();
        self.with_session(move |stream| {
            ensure_dir_cached(stream, &ensured, &assets_dir)?;
            ensure_dir_cached(stream, &ensured, &sounds_dir)?;
            write_file(stream, &path, &payload)
        })
        .await
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        let path = self.remote_path(&format!("assets/sounds/{hash}.{extension}"));
        self.with_session(move |stream| match stream.retr_as_buffer(&path) {
            Ok(buf) => Ok(Some(buf.into_inner())),
            Err(err) if is_not_found(&err) => Ok(None),
            Err(err) => Err(SyncError::network(format!("RETR sound asset: {err}"))),
        })
        .await
    }
}

// ─────────────────────────────────────────────────────────────────
// Session reuse
// ─────────────────────────────────────────────────────────────────

/// Open + login + `TYPE I` a fresh control connection. Factored
/// out of `with_session` so the reuse layer can reconnect
/// mid-call.
fn open_session(
    host: &str,
    port: u16,
    user: &str,
    password: &str,
    mode: FtpsMode,
    tls: Arc<ClientConfig>,
) -> SyncResult<SessionStream> {
    let addr = format!("{host}:{port}");
    let mut stream = match mode {
        FtpsMode::Explicit => {
            let connector = RustlsConnector::from(tls);
            let plain = RustlsFtpStream::connect(&addr)
                .map_err(|err| SyncError::network(format!("connect {addr}: {err}")))?;
            let secured = plain
                .into_secure(connector, host)
                .map_err(|err| SyncError::network(format!("AUTH TLS to {host}: {err}")))?;
            SessionStream::Tls(secured)
        }
        FtpsMode::Implicit => {
            let connector = RustlsConnector::from(tls);
            let secured = RustlsFtpStream::connect_secure_implicit(&addr, connector, host)
                .map_err(|err| SyncError::network(format!("implicit TLS connect {addr}: {err}")))?;
            SessionStream::Tls(secured)
        }
        FtpsMode::Plain => {
            let plain = FtpStream::connect(&addr)
                .map_err(|err| SyncError::network(format!("plain connect {addr}: {err}")))?;
            SessionStream::Plain(plain)
        }
    };

    stream.login(user, password).map_err(ftp_to_sync_auth)?;
    stream
        .transfer_type(FileType::Binary)
        .map_err(|err| SyncError::protocol(format!("TYPE I: {err}")))?;
    Ok(stream)
}

/// Take the parked session if it was parked less than `ttl` ago.
/// An expired session is dropped WITHOUT `QUIT` — its socket has
/// most likely been reaped by a NAT gateway already, and writing
/// a QUIT to a dead peer just wastes time; dropping sends a
/// plain FIN. Generic over the session type so the policy is
/// unit-testable without a live connection.
fn take_live<S>(slot: &Mutex<Option<(S, Instant)>>, now: Instant, ttl: Duration) -> Option<S> {
    let mut guard = slot.lock().expect("session slot mutex poison");
    match guard.take() {
        Some((session, parked_at)) if now.duration_since(parked_at) < ttl => Some(session),
        _ => None,
    }
}

/// Park a healthy session for the next call. Overwrites (drops)
/// a session another concurrent call parked meanwhile — one
/// spare connection lost, which matches the "at most one idle
/// session" policy and keeps the slot logic trivial.
fn park<S>(slot: &Mutex<Option<(S, Instant)>>, session: S, now: Instant) {
    *slot.lock().expect("session slot mutex poison") = Some((session, now));
}

/// The reuse-and-retry driver around one unit of work:
///
/// 1. Reuse the parked session when warm, else `connect`.
/// 2. On success, park the session for the next call.
/// 3. On failure of a REUSED session only: `invalidate`, then
///    retry ONCE on a fresh connection. A reused socket may have
///    died since it was parked, and the closure's error can't
///    distinguish that from a genuine server refusal once folded
///    into `SyncError` — retrying either case once is safe
///    because every trait operation is idempotent, and a genuine
///    error simply fails again and surfaces. A FRESH connection's
///    failure is never retried.
///
/// Failed sessions go through `discard`, never back into the
/// slot. Generic over the session type so the policy is
/// unit-testable with counters instead of sockets.
fn run_with_reuse<S, T>(
    slot: &Mutex<Option<(S, Instant)>>,
    ttl: Duration,
    connect: impl Fn() -> SyncResult<S>,
    discard: impl Fn(S),
    invalidate_before_retry: impl Fn(),
    mut work: impl FnMut(&mut S) -> SyncResult<T>,
) -> SyncResult<T> {
    let cached = take_live(slot, Instant::now(), ttl);
    let from_cache = cached.is_some();
    let mut session = match cached {
        Some(s) => s,
        None => connect()?,
    };
    match work(&mut session) {
        Ok(value) => {
            park(slot, session, Instant::now());
            Ok(value)
        }
        Err(first_err) => {
            discard(session);
            if !from_cache {
                return Err(first_err);
            }
            invalidate_before_retry();
            let mut fresh = connect()?;
            match work(&mut fresh) {
                Ok(value) => {
                    park(slot, fresh, Instant::now());
                    Ok(value)
                }
                Err(err) => {
                    discard(fresh);
                    Err(err)
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Sync helpers — invoked from inside the blocking closure
// ─────────────────────────────────────────────────────────────────

/// Ensure `dir` exists on the remote, at most one `MKD` per path
/// per session: consult the shared `ensured` cache first, record
/// the path after the MKD (or its "already exists" reply)
/// succeeds. A failed MKD is NOT cached, so the next call
/// retries. Sync because it runs inside `spawn_blocking`.
fn ensure_dir_cached(
    stream: &mut SessionStream,
    ensured: &Mutex<HashSet<String>>,
    dir: &str,
) -> SyncResult<()> {
    ensure_dir_once(ensured, dir, || match stream.mkdir(dir) {
        Ok(()) => Ok(()),
        Err(err) if is_already_exists(&err) => Ok(()),
        Err(err) => Err(SyncError::network(format!("MKD {dir}: {err}"))),
    })
}

/// The cache half of [`ensure_dir_cached`], generic over the
/// actual directory creation so the once-per-session behaviour
/// is unit-testable without a connection.
fn ensure_dir_once(
    ensured: &Mutex<HashSet<String>>,
    dir: &str,
    mkdir: impl FnOnce() -> SyncResult<()>,
) -> SyncResult<()> {
    if ensured.lock().expect("ensured mutex poison").contains(dir) {
        return Ok(());
    }
    mkdir()?;
    ensured
        .lock()
        .expect("ensured mutex poison")
        .insert(dir.to_string());
    Ok(())
}

/// `STOR` the given bytes to `path`.
fn write_file(stream: &mut SessionStream, path: &str, bytes: &[u8]) -> SyncResult<()> {
    let mut cursor = std::io::Cursor::new(bytes);
    stream
        .put_file(path, &mut cursor)
        .map_err(|err| SyncError::network(format!("STOR {path}: {err}")))?;
    Ok(())
}

/// `STOR` to a temp path then `RNFR`/`RNTO` over the canonical
/// name. The rename targets the destination directly — RFC 959
/// permits `RNTO` to replace an existing file — so on compliant
/// servers the canonical path never goes missing (a peer's
/// concurrent `fetch_meta` reading `Ok(None)` would be treated
/// as "fresh dataset" and bypass the engine's gates). Only an
/// overwrite-refusal reply triggers the `DELE` + one-retry
/// fallback, where the tiny missing-file window is unavoidable
/// in FTP. Most servers implement RNTO as a single inode
/// rename, which is atomic in practice even though RFC 959
/// doesn't promise it.
fn atomic_write(stream: &mut SessionStream, path: &str, bytes: &[u8]) -> SyncResult<()> {
    let tmp = format!("{path}.tmp");
    write_file(stream, &tmp, bytes)?;
    // `rename` is generic over `S: AsRef<str>` with both args
    // sharing the same type parameter — pin both to `&str` so
    // the &String/&str mismatch doesn't bite us.
    let from: &str = &tmp;
    match stream.rename(from, path) {
        Ok(()) => Ok(()),
        Err(err) if is_overwrite_refusal(&err) => {
            // Overwrite-refusing server: drop the destination and
            // retry ONCE. The DELE is best-effort — if it fails
            // the rename retry surfaces the real error anyway.
            let _ = stream.rm(path);
            stream.rename(from, path).map_err(|err| {
                SyncError::network(format!("RNFR/RNTO {tmp}->{path} after DELE: {err}"))
            })
        }
        Err(err) => Err(SyncError::network(format!(
            "RNFR/RNTO {tmp}->{path}: {err}"
        ))),
    }
}

/// Probe (once) whether the server supports RFC 3659 `MLSD`,
/// caching the verdict on the adapter. Only a delivered verdict
/// is cached — see [`mlsd_feat_verdict`]; anything else falls
/// back to NLST for THIS round only, so the next round re-probes
/// (the adapter instance is long-lived, and a transient 421
/// permanently downgrading it to NLST+SIZE would cost every
/// future round extra round trips).
fn mlsd_advertised(stream: &mut SessionStream, cache: &Mutex<Option<bool>>) -> bool {
    if let Some(known) = *cache.lock().expect("mlsd cache mutex poison") {
        return known;
    }
    match mlsd_feat_verdict(&stream.feat()) {
        Some(verdict) => {
            *cache.lock().expect("mlsd cache mutex poison") = Some(verdict);
            verdict
        }
        None => false,
    }
}

/// The cacheable half of [`mlsd_advertised`]: `Some(verdict)`
/// when the server actually delivered one — a FEAT listing, or
/// the definitive command-unknown replies (500 `BadCommand` /
/// 502 `NotImplemented`, meaning the server answered FEAT and
/// doesn't know it). `None` for everything else (421 shutdown,
/// transient 4xx, garbled reply, transport failure): the verdict
/// was never delivered, so it must not be cached.
fn mlsd_feat_verdict(feat: &FtpResult<Features>) -> Option<bool> {
    match feat {
        Ok(features) => Some(features_advertise_mlsd(features)),
        Err(FtpError::UnexpectedResponse(response))
            if matches!(response.status, Status::BadCommand | Status::NotImplemented) =>
        {
            Some(false)
        }
        Err(_) => None,
    }
}

// ─────────────────────────────────────────────────────────────────
// Pure helpers
// ─────────────────────────────────────────────────────────────────

/// How a failed per-file `RETR` in `fetch_new_logs` is handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetrDisposition {
    /// 550 with absence CONFIRMED by a follow-up `SIZE` probe:
    /// the compactor deleted the file between the listing and
    /// the RETR. Skip; next round's listing no longer carries it.
    SkipMissing,
    /// Anything else fails the WHOLE fetch. The orchestrator
    /// advances the cursor to the newest RETURNED log, so a
    /// silently-skipped file would fall below the cursor and its
    /// events would be lost permanently once its peer rotates.
    /// Failing keeps the cursor untouched — the caller serves
    /// stale data and retries next round (the WebDAV contract).
    FailBatch,
}

/// Classify a per-file `RETR` failure — see [`RetrDisposition`].
///
/// RFC 959's 550 is a catch-all ("requested action not taken;
/// file unavailable") that real servers use for permission
/// denials and locked files as much as for not-found — vsftpd
/// answers the literal same "550 Failed to open file." for both
/// — so a bare 550 must never be trusted as absence. `size_probe`
/// runs a same-session `SIZE` on the identical path, only in
/// this rare error branch: a probe that itself reports not-found
/// confirms the file is gone (the compactor race); a probe that
/// succeeds (file still listed and present) or fails any other
/// way (absence unknowable, incl. 502 SIZE-unsupported) fails
/// the batch. The bias is deliberate: misclassifying a genuine
/// race as FailBatch costs one round and self-heals (the next
/// listing no longer carries the file), while misclassifying a
/// withheld file as missing loses its events permanently. This
/// mirrors SFTP (NoSuchFile-only) and WebDAV (404-only).
fn retr_error_disposition(
    err: &FtpError,
    size_probe: impl FnOnce() -> FtpResult<usize>,
) -> RetrDisposition {
    if !is_not_found(err) {
        return RetrDisposition::FailBatch;
    }
    match size_probe() {
        Err(probe_err) if is_not_found(&probe_err) => RetrDisposition::SkipMissing,
        _ => RetrDisposition::FailBatch,
    }
}

/// Apply the cursor's size-aware filter to a listing of
/// `(basename, listed byte size)` entries. Non-log names
/// (temp files, editor backups) are skipped; everything else
/// goes through `DeviceCursor::wants_sized`, so a grown live
/// session file at/below the cursor is re-selected. Sizes are
/// the RAW remote byte counts (see the caller's E2E note).
fn select_wanted(entries: &[(String, Option<u64>)], cursor: &DeviceCursor) -> Vec<LogFileName> {
    let mut wanted = Vec::new();
    for (name, size) in entries {
        let parsed = match LogFileName::from_filename(name) {
            Ok(p) => p,
            Err(_) => {
                debug!(name = %name, "skipping non-log entry in listing");
                continue;
            }
        };
        if cursor.wants_sized(&parsed, name, *size) {
            wanted.push(parsed);
        }
    }
    wanted
}

/// Whether the `NLST` fallback should spend a `SIZE` round trip
/// on this file: only where the growth check can actually use
/// the answer — at/below the cursor (above it the file is
/// fetched regardless), not from the excluded device (never
/// fetched), and with a recorded applied length (without one,
/// `wants_sized` can't trigger a growth refetch). In practice
/// that's the 1-3 live session files of the peer devices.
fn needs_size_probe(cursor: &DeviceCursor, parsed: &LogFileName, filename: &str) -> bool {
    if cursor.wants(parsed) {
        return false;
    }
    if cursor.exclude_device.as_ref() == Some(&parsed.device_id) {
        return false;
    }
    cursor.known_lengths.iter().any(|k| k.name == filename)
}

/// Parse one `MLSD` fact line into `(basename, listed size)`.
///
/// Deliberately a local, tolerant parser rather than suppaftp's
/// `MlsxFile::from_mlsx_line`: suppaftp rejects the ENTIRE line
/// when any single fact fails its strict parse, and real servers
/// trip that constantly — ProFTPD emits four-digit octal
/// `UNIX.mode=0755` on every entry, RFC 3659 permits fractional
/// `modify` seconds, and `type=OS.unix=slink:…` values exist in
/// the wild. A line-fatal parse here would silently empty the
/// log listing and read as "no peer data" forever. Only the
/// `type` and `size` facts matter for selection, so extract
/// exactly those and ignore everything else — an unparseable
/// fact must never reject the line.
///
/// Returns `None` for directories, `cdir`/`pdir` rows and lines
/// without a pathname — the selection step never sees them. The
/// size is `None` when the server omitted the `size` fact (or
/// its value didn't parse); the growth check then degrades to
/// the plain cursor filter, same as an adapter whose listing
/// carries no sizes at all.
fn parse_mlsx_entry(line: &str) -> Option<(String, Option<u64>)> {
    // RFC 3659: `entry = [ facts ] SP pathname`, every fact ends
    // with ";" — so the pathname follows the first "; ". Fall
    // back to the first bare space for servers that drop the
    // final semicolon.
    let (facts, raw_name) = line.split_once("; ").or_else(|| line.split_once(' '))?;
    let name = basename(raw_name);
    if name.is_empty() {
        return None;
    }
    let mut size: Option<u64> = None;
    for fact in facts.split(';') {
        // A fact without '=' is malformed — skip it, never fail
        // the line over it.
        let Some((key, value)) = fact.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            // An absent `type` fact means "file" (suppaftp reads
            // it the same way); any explicit non-file value —
            // dir/cdir/pdir/link/OS.unix=… — is not a log
            // candidate.
            "type" if !value.eq_ignore_ascii_case("file") => return None,
            "size" => size = value.parse::<u64>().ok(),
            _ => {}
        }
    }
    Some((name.to_string(), size))
}

/// Whether a FEAT response advertises RFC 3659 machine listings.
/// Servers announce the capability under the `MLST` label
/// (listing the supported facts); some add an `MLSD` line too.
/// FEAT labels are case-insensitive per RFC 2389.
fn features_advertise_mlsd(features: &Features) -> bool {
    features
        .keys()
        .any(|k| k.eq_ignore_ascii_case("MLST") || k.eq_ignore_ascii_case("MLSD"))
}

/// Strip any directory prefix from a listing entry. Servers
/// disagree on whether NLST/MLSD names carry the full path.
fn basename(raw: &str) -> &str {
    raw.rsplit('/').find(|s| !s.is_empty()).unwrap_or(raw)
}

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

/// `RNTO` refusal replies for an existing destination: 550
/// (action not taken / overwrite forbidden) and 553 (file name
/// not allowed). RFC 959 permits `RNTO` to replace an existing
/// file, but a minority of servers refuse — only those replies
/// justify `atomic_write`'s DELE + one rename retry (which
/// re-opens the tiny no-`meta.json` window that rename-first
/// exists to avoid). Everything else propagates unchanged.
fn is_overwrite_refusal(err: &FtpError) -> bool {
    matches!(
        err,
        FtpError::UnexpectedResponse(response)
            if matches!(response.status, Status::FileUnavailable | Status::BadFilename)
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
    use chrono::{TimeZone, Utc};
    use std::sync::atomic::{AtomicU32, Ordering};
    use suppaftp::types::Response;
    use sync_core::{DeviceId, KnownLogLength};

    fn log_name(ts_secs: i64, device: &str) -> LogFileName {
        LogFileName::new(
            Utc.timestamp_opt(ts_secs, 0).unwrap(),
            DeviceId::from_string(device.into()),
        )
    }

    fn response_err(status: Status) -> FtpError {
        FtpError::UnexpectedResponse(Response {
            status,
            body: b"reply".to_vec(),
        })
    }

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
        assert_eq!(remote_path_for("/aperio", "meta.json"), "/aperio/meta.json",);
        assert_eq!(
            remote_path_for("/aperio", "log/2026-01-01T00:00:00Z_dev-a.jsonl",),
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

    // ── Selection (append-miss fix) ──────────────────────────────

    /// Mirrors cal-adapter-local's
    /// `grown_file_at_the_cursor_is_refetched`: a peer's live
    /// session file gains events AFTER we applied it; its
    /// timestamp sits at the cursor, but the listed size exceeds
    /// the recorded applied length, so it must be selected again.
    #[test]
    fn select_wanted_refetches_grown_file_at_the_cursor() {
        let name = log_name(1_000, "dev-a");
        let filename = name.to_filename();
        let cursor_at = |known_len: u64| DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: vec![KnownLogLength {
                name: filename.clone(),
                len: known_len,
            }],
        };

        let listed = vec![(filename.clone(), Some(120))];
        // Applied length smaller than the listing → grown → selected.
        assert_eq!(select_wanted(&listed, &cursor_at(100)).len(), 1);
        // Applied length equals the listing → unchanged → skipped.
        assert!(select_wanted(&listed, &cursor_at(120)).is_empty());
        // No listed size (NLST without a SIZE probe) → plain
        // cursor semantics: at the cursor means skipped.
        let unsized_entries = vec![(filename.clone(), None)];
        assert!(select_wanted(&unsized_entries, &cursor_at(100)).is_empty());
    }

    #[test]
    fn select_wanted_applies_cursor_exclusion_and_skips_garbage() {
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: Vec::new(),
        };
        let newer = log_name(2_000, "peer").to_filename();
        let entries = vec![
            (newer.clone(), None),
            // Own device — excluded even though newer.
            (log_name(2_000, "me").to_filename(), None),
            // Below the horizon.
            (log_name(500, "peer").to_filename(), None),
            // Not a log filename at all.
            ("editor-backup.txt~".to_string(), Some(10)),
        ];
        let wanted = select_wanted(&entries, &cursor);
        assert_eq!(wanted.len(), 1);
        assert_eq!(wanted[0].to_filename(), newer);
    }

    #[test]
    fn size_probe_only_for_known_length_names_at_or_below_the_cursor() {
        let known = log_name(1_000, "peer");
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: vec![KnownLogLength {
                name: known.to_filename(),
                len: 100,
            }],
        };
        // At the cursor with a recorded length → probe.
        assert!(needs_size_probe(&cursor, &known, &known.to_filename()));
        // Above the cursor → fetched regardless, no probe.
        let newer = log_name(2_000, "peer");
        assert!(!needs_size_probe(&cursor, &newer, &newer.to_filename()));
        // Below the cursor without a recorded length → probe is
        // useless (wants_sized can't trigger without a baseline).
        let unknown = log_name(500, "peer");
        assert!(!needs_size_probe(&cursor, &unknown, &unknown.to_filename()));
        // Own device → never fetched, never probed.
        let own = log_name(1_000, "me");
        let mut with_own_len = cursor.clone();
        with_own_len.known_lengths.push(KnownLogLength {
            name: own.to_filename(),
            len: 1,
        });
        assert!(!needs_size_probe(&with_own_len, &own, &own.to_filename()));
    }

    // ── MLSX parsing ─────────────────────────────────────────────

    #[test]
    fn parse_mlsx_entry_extracts_basename_and_raw_size() {
        let line = "type=file;size=8192;modify=20260101000000; 2026-01-01T00-00-00Z_dev-a.jsonl";
        assert_eq!(
            parse_mlsx_entry(line),
            Some(("2026-01-01T00-00-00Z_dev-a.jsonl".to_string(), Some(8192))),
        );
        // Uppercase facts (some servers) parse the same.
        let upper = "Type=file;Size=42;Modify=20260101000000; a.jsonl";
        assert_eq!(
            parse_mlsx_entry(upper),
            Some(("a.jsonl".to_string(), Some(42)))
        );
    }

    #[test]
    fn parse_mlsx_entry_without_size_fact_yields_unknown_size() {
        let line = "type=file;modify=20260101000000; 2026-01-01T00-00-00Z_dev-a.jsonl";
        assert_eq!(
            parse_mlsx_entry(line),
            Some(("2026-01-01T00-00-00Z_dev-a.jsonl".to_string(), None)),
        );
    }

    #[test]
    fn parse_mlsx_entry_skips_directories_and_garbage() {
        // Sub-directory rows.
        assert_eq!(
            parse_mlsx_entry("type=dir;modify=20260101000000; nested"),
            None
        );
        // The listed directory itself + its parent.
        assert_eq!(parse_mlsx_entry("type=cdir;modify=20260101000000; ."), None);
        assert_eq!(
            parse_mlsx_entry("type=pdir;modify=20260101000000; .."),
            None
        );
        // Degenerate lines.
        assert_eq!(parse_mlsx_entry(""), None);
    }

    /// Real-world server dialects that suppaftp's line-fatal
    /// parser rejects wholesale (any one strict-parse failure
    /// dropped the ENTIRE line, silently emptying the listing on
    /// ProFTPD-class servers). The tolerant parser must ignore
    /// the facts it doesn't need.
    #[test]
    fn parse_mlsx_entry_tolerates_real_world_fact_dialects() {
        // ProFTPD mod_facts: four-digit octal UNIX.mode on every
        // entry, plus UNIX.owner/group facts.
        let proftpd = "modify=20080820052905;perm=adfr;size=8192;type=file;\
                       unique=800U246EB03;UNIX.group=500;UNIX.mode=0644;\
                       UNIX.owner=500; 2026-01-01T00-00-00Z_dev-a.jsonl";
        assert_eq!(
            parse_mlsx_entry(proftpd),
            Some(("2026-01-01T00-00-00Z_dev-a.jsonl".to_string(), Some(8192))),
        );
        // The matching cdir row (ProFTPD's documented shape) is
        // still recognised as a directory despite UNIX.mode=0755.
        let proftpd_cdir = "modify=20080820052905;perm=fle;type=cdir;\
                            unique=800U246EB03;UNIX.group=500;UNIX.mode=0755; .";
        assert_eq!(parse_mlsx_entry(proftpd_cdir), None);
        // RFC 3659 permits fractional seconds in `modify`.
        let fractional = "type=file;size=42;modify=20260101000000.123; a.jsonl";
        assert_eq!(
            parse_mlsx_entry(fractional),
            Some(("a.jsonl".to_string(), Some(42)))
        );
        // RFC 3659 OS-specific types: a symlink is not a log file.
        let slink = "type=OS.unix=slink:/target;size=4;modify=20260101000000; link.jsonl";
        assert_eq!(parse_mlsx_entry(slink), None);
        // An unparseable size value degrades to "size unknown"
        // instead of dropping the line.
        let bad_size = "type=file;size=oops;modify=20260101000000; b.jsonl";
        assert_eq!(
            parse_mlsx_entry(bad_size),
            Some(("b.jsonl".to_string(), None))
        );
    }

    #[test]
    fn features_advertise_mlsd_matches_either_label_case_insensitively() {
        let mut features = Features::new();
        assert!(!features_advertise_mlsd(&features));
        features.insert("SIZE".to_string(), None);
        assert!(!features_advertise_mlsd(&features));
        features.insert(
            "MLST".to_string(),
            Some("size*;create;modify*;perm".to_string()),
        );
        assert!(features_advertise_mlsd(&features));

        let mut lowercase = Features::new();
        lowercase.insert("mlsd".to_string(), None);
        assert!(features_advertise_mlsd(&lowercase));
    }

    #[test]
    fn mlsd_verdict_caches_only_delivered_answers() {
        // A FEAT listing is a delivered verdict either way.
        let mut features = Features::new();
        features.insert("MLST".to_string(), None);
        assert_eq!(mlsd_feat_verdict(&Ok(features)), Some(true));
        assert_eq!(mlsd_feat_verdict(&Ok(Features::new())), Some(false));
        // 500/502: the server answered FEAT and doesn't know it —
        // a definitive "no", safe to cache.
        assert_eq!(
            mlsd_feat_verdict(&Err(response_err(Status::BadCommand))),
            Some(false),
        );
        assert_eq!(
            mlsd_feat_verdict(&Err(response_err(Status::NotImplemented))),
            Some(false),
        );
        // 421 shutdown / transport failure: no verdict was ever
        // delivered — caching "no MLSD" here would downgrade a
        // capable server to NLST+SIZE for the adapter's lifetime.
        assert_eq!(
            mlsd_feat_verdict(&Err(response_err(Status::NotAvailable))),
            None,
        );
        assert_eq!(
            mlsd_feat_verdict(&Err(FtpError::ConnectionError(std::io::Error::other(
                "connection reset",
            )))),
            None,
        );
        assert_eq!(mlsd_feat_verdict(&Err(FtpError::BadResponse)), None);
    }

    #[test]
    fn basename_strips_optional_directory_prefixes() {
        assert_eq!(basename("a.jsonl"), "a.jsonl");
        assert_eq!(basename("/aperio/log/a.jsonl"), "a.jsonl");
        assert_eq!(basename("log/a.jsonl"), "a.jsonl");
    }

    // ── Error dispositions ───────────────────────────────────────

    #[test]
    fn retr_550_skips_only_when_the_size_probe_confirms_absence() {
        // Probe-confirmed compactor race: listed, deleted before
        // the RETR, and SIZE agrees the file is gone.
        assert_eq!(
            retr_error_disposition(&response_err(Status::FileUnavailable), || Err(
                response_err(Status::FileUnavailable)
            )),
            RetrDisposition::SkipMissing,
        );
        // SIZE succeeds → the file still exists, so the 550 was a
        // permission/lock refusal (RFC 959's 550 is a catch-all).
        // Skipping would advance the cursor past live events.
        assert_eq!(
            retr_error_disposition(&response_err(Status::FileUnavailable), || Ok(8192)),
            RetrDisposition::FailBatch,
        );
        // SIZE fails any other way → absence unknowable → fail.
        assert_eq!(
            retr_error_disposition(&response_err(Status::FileUnavailable), || Err(
                response_err(Status::NotImplemented)
            )),
            RetrDisposition::FailBatch,
        );
        assert_eq!(
            retr_error_disposition(&response_err(Status::FileUnavailable), || Err(
                FtpError::ConnectionError(std::io::Error::other("connection reset"))
            )),
            RetrDisposition::FailBatch,
        );
    }

    #[test]
    fn retr_non_550_fails_the_batch_without_probing() {
        // Any non-550 refusal fails the whole fetch so the cursor
        // never advances past a withheld file — and the SIZE
        // probe must not even run.
        let no_probe = || -> FtpResult<usize> { panic!("probe must not run for non-550") };
        assert_eq!(
            retr_error_disposition(&response_err(Status::NotLoggedIn), no_probe),
            RetrDisposition::FailBatch,
        );
        let io_err = FtpError::ConnectionError(std::io::Error::other("connection reset"));
        assert_eq!(
            retr_error_disposition(&io_err, no_probe),
            RetrDisposition::FailBatch
        );
    }

    #[test]
    fn overwrite_refusal_matches_550_and_553_replies_only() {
        assert!(is_overwrite_refusal(&response_err(Status::FileUnavailable)));
        assert!(is_overwrite_refusal(&response_err(Status::BadFilename)));
        // Not refusals: auth failures + transport errors propagate.
        assert!(!is_overwrite_refusal(&response_err(Status::NotLoggedIn)));
        assert!(!is_overwrite_refusal(&FtpError::ConnectionError(
            std::io::Error::other("connection reset"),
        )));
    }

    // ── Session reuse policy ─────────────────────────────────────

    #[test]
    fn parked_session_is_reused_within_ttl_and_dropped_after() {
        let slot: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
        let t0 = Instant::now();
        assert_eq!(take_live(&slot, t0, SESSION_IDLE_TTL), None);

        park(&slot, 7, t0);
        // Warm: within the TTL the parked session comes back …
        assert_eq!(
            take_live(&slot, t0 + Duration::from_secs(1), SESSION_IDLE_TTL),
            Some(7),
        );
        // … and taking empties the slot.
        assert_eq!(
            take_live(&slot, t0 + Duration::from_secs(1), SESSION_IDLE_TTL),
            None,
        );

        // Stale: past the TTL the session is dropped, never reused.
        park(&slot, 8, t0);
        assert_eq!(
            take_live(&slot, t0 + Duration::from_secs(10), SESSION_IDLE_TTL),
            None,
        );
    }

    #[test]
    fn back_to_back_calls_share_one_connection() {
        let slot: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
        let connects = AtomicU32::new(0);
        let connect = || -> SyncResult<u32> {
            connects.fetch_add(1, Ordering::SeqCst);
            Ok(0)
        };
        // A push round fires several trait calls back-to-back;
        // they must all ride the first call's connection.
        for _ in 0..4 {
            run_with_reuse(
                &slot,
                SESSION_IDLE_TTL,
                connect,
                |_s| {},
                || {},
                |_s| Ok(()),
            )
            .unwrap();
        }
        assert_eq!(connects.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_on_a_reused_session_gets_exactly_one_fresh_retry() {
        let slot: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
        // Park a session whose work will fail — the dead-socket case.
        park(&slot, 1, Instant::now());
        let connects = AtomicU32::new(0);
        let attempts = AtomicU32::new(0);
        let invalidated = AtomicU32::new(0);
        let result = run_with_reuse(
            &slot,
            SESSION_IDLE_TTL,
            || -> SyncResult<u32> {
                connects.fetch_add(1, Ordering::SeqCst);
                Ok(2)
            },
            |_s| {},
            || {
                invalidated.fetch_add(1, Ordering::SeqCst);
            },
            |s: &mut u32| {
                attempts.fetch_add(1, Ordering::SeqCst);
                if *s == 1 {
                    Err(SyncError::network("dead socket"))
                } else {
                    Ok(*s)
                }
            },
        );
        // The retry ran on the fresh connection and succeeded.
        assert_eq!(result.unwrap(), 2);
        assert_eq!(connects.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        // The ensured-directory cache was invalidated before the retry.
        assert_eq!(invalidated.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn failure_on_a_fresh_session_is_not_retried() {
        let slot: Mutex<Option<(u32, Instant)>> = Mutex::new(None);
        let connects = AtomicU32::new(0);
        let attempts = AtomicU32::new(0);
        let err = run_with_reuse(
            &slot,
            SESSION_IDLE_TTL,
            || -> SyncResult<u32> {
                connects.fetch_add(1, Ordering::SeqCst);
                Ok(9)
            },
            |_s| {},
            || panic!("no invalidation without a retry"),
            |_s: &mut u32| -> SyncResult<()> {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(SyncError::network("server refused"))
            },
        )
        .unwrap_err();
        assert!(matches!(err, SyncError::Network(_)));
        assert_eq!(connects.load(Ordering::SeqCst), 1);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
        // A failed session is never parked back.
        assert!(take_live(&slot, Instant::now(), SESSION_IDLE_TTL).is_none());
    }

    // ── Ensured-directory cache ──────────────────────────────────

    #[test]
    fn ensured_directory_is_created_only_once_per_session() {
        // Mirrors webdav's push_log_mkcols_the_collection_only_once_per_session:
        // three pushes, exactly one directory creation.
        let ensured = Mutex::new(HashSet::new());
        let mkdirs = AtomicU32::new(0);
        for _ in 0..3 {
            ensure_dir_once(&ensured, "/aperio/log", || {
                mkdirs.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(mkdirs.load(Ordering::SeqCst), 1);
        // Distinct paths each get their own MKD.
        ensure_dir_once(&ensured, "/aperio/assets", || {
            mkdirs.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
        assert_eq!(mkdirs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_directory_creation_is_not_cached() {
        let ensured = Mutex::new(HashSet::new());
        let calls = AtomicU32::new(0);
        let failing = || -> SyncResult<()> {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(SyncError::network("MKD refused"))
        };
        assert!(ensure_dir_once(&ensured, "/x", failing).is_err());
        // The failure wasn't recorded — the next attempt retries.
        assert!(ensure_dir_once(&ensured, "/x", failing).is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
