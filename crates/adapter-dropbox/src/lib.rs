//! Dropbox API v2 `SyncAdapter` implementation (DESIGN.md §19.6).
//!
//! Pure Rust HTTP client built on `reqwest` (rustls feature, no
//! system OpenSSL needed — Mobile-tauglich per §19.6). OAuth 2.0
//! PKCE authorisation-code flow against the user's own Dropbox
//! app (the user creates one at dropbox.com/developers/apps and
//! supplies the client_id + client_secret); refresh tokens live
//! in the OS keychain.
//!
//! The adapter takes a refresh token at construction and lazily
//! mints fresh access tokens whenever the cached one expires or
//! the API answers 401. The OAuth dance (browser launch +
//! loopback redirect) lives in [`oauth`] and is exposed via
//! [`oauth::run`] / [`oauth::refresh`] — typically driven from
//! the Tauri command layer's "Sign in with Dropbox" button
//! before the user clicks the regular "Connect" action.
//!
//! Maps the [`SyncAdapter`] trait onto Dropbox API v2 endpoints:
//!
//! | Trait call           | Dropbox endpoint                         |
//! |----------------------|------------------------------------------|
//! | `test_connection`    | `POST /2/check/user` + `MKD` lazily      |
//! | `fetch_meta`         | `POST /2/files/download` of `meta.json`  |
//! | `push_meta`          | `POST /2/files/upload` (mode=overwrite)  |
//! | `fetch_new_logs`     | `POST /2/files/list_folder` + per-file   |
//! |                      | `download`                                |
//! | `push_log`           | `POST /2/files/upload`                   |
//! | `fetch/push_snap`    | `download` / `upload` over snapshot.json |
//! | `delete_log`         | `POST /2/files/delete_v2`                |
//! | sound asset CRUD     | `<base>/assets/sounds/<hash>.<ext>`      |
//!
//! ## Atomic writes
//!
//! `meta.json` + `snapshot.json` upload with
//! `mode: "overwrite"` so the new content lands at the canonical
//! path in one round-trip — Dropbox handles the replace
//! atomically server-side, so we don't need the tmp + rename
//! dance that FTP requires.
//!
//! ## Path format
//!
//! Dropbox paths are case-insensitive and always start with `/`.
//! Empty string addresses the app's root folder (when the app has
//! "App folder" scope) or the user's whole Dropbox (when the app
//! has "Full Dropbox" scope). The adapter normalises the base
//! path to `/foo/bar` (leading slash, no trailing) at
//! construction; relative paths are joined with `/`.
//!
//! ## What this crate does NOT do
//!
//! - **Cursor-based incremental sync.** Each `fetch_new_logs`
//!   round walks the whole `log/` folder via list_folder and
//!   filters client-side by timestamp + listed size (the size
//!   feeds the growth-refetch check). Dropbox offers
//!   `list_folder/continue` for delta sync; v1 doesn't use it
//!   because the log folder typically holds < 1000 entries
//!   (compaction keeps it small).
//! - **Long-lived tokens.** Each adapter instance keeps its own
//!   `Mutex<TokenSet>` and refreshes on demand. No background
//!   token-refresh task.
//! - **App-folder vs. full-Dropbox scope discrimination.** Both
//!   work; the adapter doesn't care which permission the user's
//!   Dropbox app holds.

pub mod error;
pub mod files;
pub mod oauth;

use std::collections::HashSet;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter, SyncError, SyncResult,
};
use tokio::sync::Mutex;
use tracing::debug;

pub use error::{DropboxError, DropboxResult};
pub use oauth::{TokenSet, DROPBOX_AUTH_URL, DROPBOX_TOKEN_URL};

// ─────────────────────────────────────────────────────────────────
// Config types
// ─────────────────────────────────────────────────────────────────

/// Persisted, non-secret Dropbox configuration. Mirrors
/// `adapter-google`'s shape — client_id + client_secret
/// come from the user's own app registration, the base path is
/// the folder inside their Dropbox that holds the dataset.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DropboxAccountConfig {
    pub client_id: String,
    /// Empty string for confidential apps that don't issue one
    /// (Dropbox supports PKCE-only public apps). The token
    /// endpoint accepts an empty client_secret when the app is
    /// configured as "Public" in the Dropbox developer console.
    #[serde(default)]
    pub client_secret: String,
    /// Remote folder, e.g. `/aperio`. The empty string means
    /// "app root" (for app-folder-scoped apps) or "Dropbox
    /// root" (for full-Dropbox apps).
    pub base_path: String,
}

/// Live Dropbox `SyncAdapter`. Holds the long-lived refresh
/// token + the user's app credentials, plus a tokio Mutex'd
/// cache of the short-lived access token. Cheap to clone — the
/// inner Arcs share state.
#[derive(Debug, Clone)]
pub struct DropboxSyncAdapter {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    base_path: String,
    http: Arc<Client>,
    tokens: Arc<Mutex<Option<TokenSet>>>,
    /// Remote folders (`log`, `assets/sounds`, …) already ensured
    /// via `create_folder_v2` this session. Dropbox folders persist
    /// server-side, so once one is ensured we skip the redundant
    /// RPC every later push would otherwise pay. Shared across
    /// clones so the cache survives `.clone()`.
    ensured: Arc<StdMutex<HashSet<String>>>,
}

impl DropboxSyncAdapter {
    /// Build an adapter from a refresh token (stored in the OS
    /// keychain after the OAuth dance completed) plus the
    /// user's app credentials.
    ///
    /// The constructor doesn't hit the network — token refresh
    /// happens lazily on the first API call. `test_connection`
    /// is the canonical "did we wire this up correctly" probe.
    pub fn new(
        config: DropboxAccountConfig,
        refresh_token: impl Into<String>,
    ) -> DropboxResult<Self> {
        if config.client_id.trim().is_empty() {
            return Err(DropboxError::Config("client_id must not be empty".into()));
        }
        Ok(Self {
            client_id: config.client_id,
            client_secret: config.client_secret,
            refresh_token: refresh_token.into(),
            base_path: normalise_base(&config.base_path),
            http: Arc::new(Client::new()),
            tokens: Arc::new(Mutex::new(None)),
            ensured: Arc::new(StdMutex::new(HashSet::new())),
        })
    }

    /// Variant that lets the caller supply a pre-built
    /// `reqwest::Client` — used by tests that want to point at
    /// a mockito server and by callers that share a single
    /// client pool across adapters.
    pub fn with_client(
        config: DropboxAccountConfig,
        refresh_token: impl Into<String>,
        http: Arc<Client>,
    ) -> DropboxResult<Self> {
        let mut adapter = Self::new(config, refresh_token)?;
        adapter.http = http;
        Ok(adapter)
    }

    /// Join `relative` onto the base path. `relative` MUST NOT
    /// start with a slash; the base is either empty (root) or
    /// `/foo/bar` with no trailing slash.
    fn remote_path(&self, relative: &str) -> String {
        remote_path_for(&self.base_path, relative)
    }

    /// Borrow a fresh-enough access token. If the cache is
    /// empty or the cached token expires within 60 seconds,
    /// run a refresh round-trip. Concurrent callers serialise
    /// on the mutex; the first one through does the refresh
    /// and the rest see the updated cache.
    async fn access_token(&self) -> SyncResult<String> {
        let mut guard = self.tokens.lock().await;
        let needs_refresh = match guard.as_ref() {
            None => true,
            Some(tok) => {
                let safety = chrono::Duration::seconds(60);
                tok.expires_at <= Utc::now() + safety
            }
        };
        if needs_refresh {
            let refreshed = oauth::refresh(
                &self.client_id,
                &self.client_secret,
                &self.refresh_token,
                &self.http,
            )
            .await
            .map_err(dropbox_to_sync)?;
            *guard = Some(refreshed);
        }
        Ok(guard
            .as_ref()
            .expect("token set was just installed")
            .access_token
            .clone())
    }

    /// Force a fresh token round-trip — used by the trait
    /// methods on a 401 retry path. `stale` is the token the
    /// caller just saw fail: when the cache already holds a
    /// DIFFERENT token, a concurrent retry refreshed it moments
    /// ago (the download batch runs several files at once, so one
    /// mid-batch expiry can 401 on all of them), and that fresh
    /// token is returned as-is instead of being discarded for yet
    /// another token round-trip. Only when the cache still holds
    /// the failed token is it dropped and re-minted via
    /// [`Self::access_token`].
    async fn force_refresh(&self, stale: &str) -> SyncResult<String> {
        {
            let mut guard = self.tokens.lock().await;
            if let Some(tok) = guard.as_ref() {
                if tok.access_token != stale {
                    return Ok(tok.access_token.clone());
                }
            }
            *guard = None;
        }
        self.access_token().await
    }

    /// Create a remote folder at most once per adapter session.
    ///
    /// Dropbox folders persist server-side, so re-issuing
    /// `create_folder_v2` before every push just burned one RPC
    /// per pushed file. `files::create_folder` folds the
    /// "already exists" conflict into Ok, so any Ok — freshly
    /// created or conflicting — proves the folder is there and
    /// caches it. Errors are NOT cached: the next push retries
    /// the ensure. A concurrent first-touch racing past the
    /// cache check at worst issues a second harmless
    /// `create_folder_v2`.
    async fn ensure_folder(&self, token: &str, path: &str) -> SyncResult<()> {
        if self
            .ensured
            .lock()
            .expect("ensured mutex poison")
            .contains(path)
        {
            return Ok(());
        }
        files::create_folder(&self.http, token, path)
            .await
            .map_err(dropbox_to_sync)?;
        self.ensured
            .lock()
            .expect("ensured mutex poison")
            .insert(path.to_string());
        Ok(())
    }
}

#[async_trait]
impl SyncAdapter for DropboxSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        let token = self.access_token().await?;
        files::check_user(&self.http, &token)
            .await
            .map_err(dropbox_to_sync)?;
        // Lazy-create the dataset folders so first push works.
        // `create_folder_v2` is idempotent in our wrapper: a
        // path/conflict response is folded into Ok. Going through
        // `ensure_folder` also seeds the session cache, so the
        // pushes that follow skip their own create_folder RPCs.
        if !self.base_path.is_empty() {
            self.ensure_folder(&token, &self.base_path).await?;
        }
        self.ensure_folder(&token, &self.remote_path("log")).await?;
        self.ensure_folder(&token, &self.remote_path("assets"))
            .await?;
        self.ensure_folder(&token, &self.remote_path("assets/sounds"))
            .await?;
        Ok(())
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let path = self.remote_path("meta.json");
        match self.download_with_retry(&path).await {
            Ok(Some(bytes)) => Ok(Some(MetaJson::from_bytes(&bytes)?)),
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let bytes = meta.to_bytes()?;
        let path = self.remote_path("meta.json");
        self.upload_with_retry(&path, bytes).await
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        let log_dir = self.remote_path("log");
        let token = self.access_token().await?;
        let entries = match files::list_folder(&self.http, &token, &log_dir).await {
            Ok(e) => e,
            // Auth retry: one shot, then surface.
            Err(err) if err.is_auth() => {
                let token = self.force_refresh(&token).await?;
                files::list_folder(&self.http, &token, &log_dir)
                    .await
                    .map_err(dropbox_to_sync)?
            }
            // `path/not_found` for the log folder itself = no
            // logs ever pushed yet (fresh dataset).
            Err(err) if err.is_not_found() => return Ok(Vec::new()),
            Err(err) => return Err(dropbox_to_sync(err)),
        };

        let wanted = select_wanted_logs(since, &entries);

        // Download with bounded concurrency: a multi-file backlog
        // (onboarding, post-offline catch-up, multi-device burst)
        // used to pay one serial content-host round-trip per file.
        // The bound stays modest so we sit far below Dropbox's
        // per-app rate limits. Out-of-order completion is fine —
        // the orchestrator sorts chronologically before apply.
        const LOG_FETCH_CONCURRENCY: usize = 4;
        let results: Vec<SyncResult<Option<LogFile>>> = stream::iter(wanted)
            .map(|parsed| {
                let path = format!("{}/{}", log_dir, parsed.to_filename());
                async move {
                    match self.download_with_retry(&path).await? {
                        Some(bytes) => Ok(Some(LogFile {
                            name: parsed,
                            bytes,
                        })),
                        None => {
                            // Compactor raced us between list_folder
                            // + download; skip silently — the next
                            // round sees an updated listing.
                            debug!(
                                path = %path,
                                "log file listed but no longer present",
                            );
                            Ok(None)
                        }
                    }
                }
            })
            .buffer_unordered(LOG_FETCH_CONCURRENCY)
            .collect()
            .await;
        // Any hard download failure fails the WHOLE fetch (the
        // caller serves stale and retries next round). Skipping
        // just the failed file would let the orchestrator advance
        // the cursor past it — its events would sit below the
        // cursor with no applied-length record, permanently lost
        // to this device.
        let mut out = Vec::with_capacity(results.len());
        for result in results {
            if let Some(log) = result? {
                out.push(log);
            }
        }
        Ok(out)
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        // Lazy-create log/ on first push so a brand-new dataset
        // doesn't bounce off path/not_found — but only once per
        // session; the ensured cache skips the redundant RPC on
        // every later push.
        let token = self.access_token().await?;
        self.ensure_folder(&token, &self.remote_path("log")).await?;
        let path = self.remote_path(&format!("log/{}", log.name.to_filename()));
        self.upload_with_retry(&path, log.bytes.clone()).await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let path = self.remote_path("snapshot.json");
        match self.download_with_retry(&path).await {
            Ok(Some(bytes)) => Ok(Some(Snapshot::from_bytes(&bytes)?)),
            Ok(None) => Ok(None),
            Err(err) => Err(err),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let bytes = snapshot.to_bytes()?;
        let path = self.remote_path("snapshot.json");
        self.upload_with_retry(&path, bytes).await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let path = self.remote_path(&format!("log/{}", name.to_filename()));
        let token = self.access_token().await?;
        let result = files::delete(&self.http, &token, &path).await;
        match result {
            Ok(()) => Ok(()),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh(&token).await?;
                match files::delete(&self.http, &token, &path).await {
                    Ok(()) => Ok(()),
                    // Not-found = goal already met — the retry arm
                    // must honour the same idempotent-delete
                    // contract as the primary arm below, or a
                    // token expiry would turn "already gone" into
                    // a spurious hard failure.
                    Err(err) if err.is_not_found() => Ok(()),
                    Err(err) => Err(dropbox_to_sync(err)),
                }
            }
            // Not-found = goal already met (file is gone); same
            // semantics as the SFTP / FTPS adapters.
            Err(err) if err.is_not_found() => Ok(()),
            Err(err) => Err(dropbox_to_sync(err)),
        }
    }

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        let token = self.access_token().await?;
        self.ensure_folder(&token, &self.remote_path("assets"))
            .await?;
        self.ensure_folder(&token, &self.remote_path("assets/sounds"))
            .await?;
        let path = self.remote_path(&format!("assets/sounds/{hash}.{extension}"));
        self.upload_with_retry(&path, bytes.to_vec()).await
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        let path = self.remote_path(&format!("assets/sounds/{hash}.{extension}"));
        self.download_with_retry(&path).await
    }
}

impl DropboxSyncAdapter {
    /// Upload helper with one-shot 401 retry: on `auth/expired`
    /// the cached token has likely drifted (e.g. server-side
    /// invalidation); refresh and retry once. Same body bytes
    /// are reused — the upload-tmp + rename dance isn't needed
    /// because Dropbox supports `mode: "overwrite"` atomically.
    async fn upload_with_retry(&self, path: &str, bytes: Vec<u8>) -> SyncResult<()> {
        let token = self.access_token().await?;
        match files::upload(&self.http, &token, path, &bytes).await {
            Ok(()) => Ok(()),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh(&token).await?;
                files::upload(&self.http, &token, path, &bytes)
                    .await
                    .map_err(dropbox_to_sync)
            }
            Err(err) => Err(dropbox_to_sync(err)),
        }
    }

    /// Download helper with the same 401 retry as upload.
    /// `Ok(None)` covers `path/not_found` — the read-side
    /// "doesn't exist" path that the upper layer folds into
    /// "fresh dataset" or "compactor raced us" branches.
    async fn download_with_retry(&self, path: &str) -> SyncResult<Option<Vec<u8>>> {
        let token = self.access_token().await?;
        match files::download(&self.http, &token, path).await {
            Ok(Some(bytes)) => Ok(Some(bytes)),
            Ok(None) => Ok(None),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh(&token).await?;
                files::download(&self.http, &token, path)
                    .await
                    .map_err(dropbox_to_sync)
            }
            Err(err) => Err(dropbox_to_sync(err)),
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// Pure helpers
// ─────────────────────────────────────────────────────────────────

/// Pick the log files the cursor wants from a raw `list_folder`
/// listing of `(basename, listed size)` pairs. Pure so the
/// selection is unit-testable without HTTP.
///
/// The size feeds the growth-refetch check (`wants_sized`): a
/// peer's live session file that gained appended events since we
/// last applied it is re-fetched even though its timestamp sits
/// at/below the cursor. Sizes are the RAW remote byte counts as
/// listed — under E2E that's ciphertext, and `EncryptingAdapter`
/// already translated the cursor's `known_lengths` into the same
/// domain before this adapter sees them, so no adjustment here.
fn select_wanted_logs(since: &DeviceCursor, entries: &[(String, Option<u64>)]) -> Vec<LogFileName> {
    let mut wanted = Vec::new();
    for (raw, size) in entries {
        // list_folder's `name` is already a basename; the rsplit
        // is defensive against a path sneaking in.
        let name = raw.rsplit('/').next().unwrap_or(raw);
        let parsed = match LogFileName::from_filename(name) {
            Ok(p) => p,
            Err(_) => {
                debug!(name = %name, "skipping non-log entry in list_folder");
                continue;
            }
        };
        if since.wants_sized(&parsed, name, *size) {
            wanted.push(parsed);
        }
    }
    wanted
}

/// Join `relative` onto `base`. `relative` MUST NOT start with
/// `/`; `base` is empty or starts with `/` without trailing slash.
/// Empty `base` + empty `relative` → empty string (Dropbox's
/// "root" address; the API rejects `"/"` for the literal root).
fn remote_path_for(base: &str, relative: &str) -> String {
    if base.is_empty() {
        if relative.is_empty() {
            String::new()
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
/// the adapter expects: empty (root) or `/foo/bar` (leading
/// slash, no trailing slash).
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

/// Map a [`DropboxError`] into the [`SyncError`] vocabulary the
/// rest of the sync layer uses. Authentication-class errors
/// surface as `Auth`; not-found as `Network` (the upper layer's
/// `Ok(None)` short-circuit happens before this fn is reached on
/// the read paths); everything else folds into `Network` since
/// the connection itself worked.
fn dropbox_to_sync(err: DropboxError) -> SyncError {
    match err {
        DropboxError::Auth(msg) => SyncError::auth(msg),
        DropboxError::NotFound(msg) => SyncError::not_found(msg),
        DropboxError::Config(msg) => SyncError::internal(msg),
        DropboxError::Protocol(msg) => SyncError::protocol(msg),
        DropboxError::Http { status, message } => {
            SyncError::network(format!("HTTP {status}: {message}"))
        }
        DropboxError::Io(msg) => SyncError::io(msg),
        DropboxError::Csrf => SyncError::auth("OAuth state mismatch (CSRF)"),
        DropboxError::AuthTimeout => SyncError::auth("OAuth dance timed out"),
        DropboxError::AuthDenied(msg) => SyncError::auth(format!("OAuth consent denied: {msg}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use sync_core::{DeviceId, KnownLogLength};

    fn log_name(ts_secs: i64, device: &str) -> LogFileName {
        LogFileName::new(
            Utc.timestamp_opt(ts_secs, 0).unwrap(),
            DeviceId::from_string(device.into()),
        )
    }

    #[test]
    fn selection_applies_cursor_horizon_and_skips_non_logs() {
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_500, 0).unwrap(),
            exclude_device: None,
            known_lengths: Vec::new(),
        };
        let newer = log_name(2_000, "peer");
        let older = log_name(1_000, "peer");
        let entries = vec![
            (newer.to_filename(), Some(10)),
            (older.to_filename(), Some(10)),
            // Stray non-log entry (temp file, editor backup) —
            // silently dropped by the filename parse.
            ("garbage.tmp".to_string(), Some(3)),
        ];
        assert_eq!(select_wanted_logs(&cursor, &entries), vec![newer]);
    }

    #[test]
    fn selection_skips_own_device_files() {
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: Vec::new(),
        };
        let own = log_name(2_000, "me");
        assert!(select_wanted_logs(&cursor, &[(own.to_filename(), Some(50))]).is_empty());
    }

    #[test]
    fn grown_file_at_the_cursor_is_selected_again() {
        // The append-miss fix, selection-level mirror of the local
        // adapter's grown_file_at_the_cursor_is_refetched: a peer's
        // live session file gained events AFTER we applied it; its
        // timestamp sits AT the cursor, but the listed size exceeds
        // the recorded applied length, so it must be fetched again.
        let file = log_name(1_000, "peer");
        let filename = file.to_filename();
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: None,
            known_lengths: vec![KnownLogLength {
                name: filename.clone(),
                len: 100,
            }],
        };
        // Listed size grew past the applied length → re-selected.
        assert_eq!(
            select_wanted_logs(&cursor, &[(filename.clone(), Some(150))]),
            vec![file],
        );
        // Unchanged → skipped.
        assert!(select_wanted_logs(&cursor, &[(filename.clone(), Some(100))]).is_empty());
        // No listed size → plain timestamp semantics, skipped.
        assert!(select_wanted_logs(&cursor, &[(filename, None)]).is_empty());
    }

    #[test]
    fn ensured_cache_is_shared_across_clones() {
        // The once-per-SESSION semantics depend on clones sharing
        // the cache — the host hands out clones of one adapter, and
        // a per-clone cache would silently regress to one
        // create_folder RPC per clone.
        let adapter = DropboxSyncAdapter::new(
            DropboxAccountConfig {
                client_id: "cid".into(),
                client_secret: String::new(),
                base_path: "/aperio".into(),
            },
            "refresh-token",
        )
        .unwrap();
        let clone = adapter.clone();
        adapter.ensured.lock().unwrap().insert("/aperio/log".into());
        assert!(clone.ensured.lock().unwrap().contains("/aperio/log"));
    }

    #[tokio::test]
    async fn force_refresh_reuses_token_installed_by_a_concurrent_retry() {
        // buffer_unordered lets one mid-batch expiry 401 several
        // downloads at once. Each retry calls force_refresh; only
        // the first may pay a token round-trip — the rest must see
        // that the cache already holds a DIFFERENT token and reuse
        // it. Were the compare missing, this test would null the
        // cache and attempt a real network refresh (and fail).
        let adapter = DropboxSyncAdapter::new(
            DropboxAccountConfig {
                client_id: "cid".into(),
                client_secret: String::new(),
                base_path: "/aperio".into(),
            },
            "refresh-token",
        )
        .unwrap();
        *adapter.tokens.lock().await = Some(TokenSet {
            access_token: "fresh".into(),
            refresh_token: None,
            expires_at: Utc::now() + chrono::Duration::hours(1),
        });
        let got = adapter.force_refresh("stale").await.unwrap();
        assert_eq!(got, "fresh");
        // And the freshly-installed token stays cached — the
        // short-circuit must not disturb the cache.
        assert_eq!(
            adapter.tokens.lock().await.as_ref().unwrap().access_token,
            "fresh",
        );
    }

    #[test]
    fn normalise_base_handles_common_shapes() {
        assert_eq!(normalise_base(""), "");
        assert_eq!(normalise_base("/"), "");
        assert_eq!(normalise_base("aperio"), "/aperio");
        assert_eq!(normalise_base("/aperio"), "/aperio");
        assert_eq!(normalise_base("/aperio/"), "/aperio");
        assert_eq!(normalise_base("aperio/"), "/aperio");
        assert_eq!(normalise_base("/path/with/slashes/"), "/path/with/slashes");
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

        // Empty base = Dropbox root. Note that `""` (NOT `"/"`)
        // is the API's address for root; remote_path_for honours
        // that.
        assert_eq!(remote_path_for("", ""), "");
        assert_eq!(remote_path_for("", "meta.json"), "/meta.json");
    }
}
