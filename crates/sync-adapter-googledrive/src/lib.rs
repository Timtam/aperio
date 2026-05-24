//! Google Drive API v3 `SyncAdapter` implementation
//! (DESIGN.md §19.6).
//!
//! Pure-Rust HTTP client built on `reqwest` (rustls) +
//! Google's OAuth 2.0 PKCE installed-app flow with the
//! `drive.file` scope — the per-app scope that only sees
//! files the app itself created. Aperio never reads or
//! modifies anything else in the user's Drive.
//!
//! ## ID-based addressing
//!
//! Drive's biggest difference from Dropbox / FTP / SFTP:
//! files don't have paths. Every file has an opaque ID and
//! refers to its parents via a `parents[]` array. The trait
//! method bodies in this file map Aperio's path semantics
//! (`meta.json`, `log/<name>.jsonl`, …) onto ID-based ops by
//! resolving + caching folder IDs at the adapter layer.
//!
//! Folder layout we cache:
//!
//! - `base_folder_id` — the "aperio" (or whatever the user
//!   named it) folder under the user's Drive root.
//! - `log_folder_id` — `log/` under base.
//! - `assets_folder_id` — `assets/` under base.
//! - `sounds_folder_id` — `assets/sounds/` under base.
//!
//! All four resolve lazily on first access via
//! `create_folder` (which is idempotent — returns the existing
//! ID when present). They live in a `Mutex<FolderIds>` so
//! concurrent trait methods don't all re-resolve.
//!
//! Individual file IDs (meta.json, snapshot.json, each log,
//! each sound asset) are NOT cached — each operation does a
//! single `find_child` round-trip to get the ID, then the
//! actual GET / DELETE / PATCH. That's one extra request per
//! op but keeps the cache invariant trivial: folder IDs are
//! stable (we never rename), file IDs change on each
//! upload (Drive doesn't reuse IDs across deletes).
//!
//! ## Atomic writes
//!
//! `meta.json` + `snapshot.json` use Drive's PATCH-content
//! flow when the file already exists: a single round-trip
//! replaces the content under the existing file ID,
//! atomically server-side. No tmp+rename dance needed —
//! same gain Dropbox gives us.

pub mod error;
pub mod files;
pub mod oauth;

use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter,
    SyncError, SyncResult,
};
use tokio::sync::Mutex;
use tracing::{debug, warn};

pub use error::{GoogleDriveError, GoogleDriveResult};
pub use oauth::{TokenSet, GOOGLE_AUTH_URL, GOOGLE_TOKEN_URL, SCOPE_DRIVE_FILE};

/// Persisted, non-secret Drive configuration. `folder_name`
/// is the human-friendly label the user enters in the
/// settings UI — typically "Aperio". The adapter creates (or
/// reuses) a folder of that name under the user's My Drive
/// root the first time `test_connection` runs and remembers
/// the resolved ID.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoogleDriveAccountConfig {
    pub client_id: String,
    pub client_secret: String,
    /// Human-readable folder name. Empty / missing defaults
    /// to "Aperio". Drive's filenames are not unique within
    /// a parent, but the adapter's create_folder helper folds
    /// "already exists" into success so re-running setup is
    /// safe.
    #[serde(default)]
    pub folder_name: String,
}

/// Resolved folder IDs cached for the adapter's lifetime.
/// Built lazily by [`DriveSyncAdapter::ensure_folder_ids`].
#[derive(Debug, Default, Clone)]
struct FolderIds {
    base: Option<String>,
    log: Option<String>,
    assets: Option<String>,
    sounds: Option<String>,
}

/// Google Drive `SyncAdapter`. Holds the app credentials +
/// refresh token + the lazily-resolved folder IDs. Cheap to
/// clone — inner state is Arc-shared.
#[derive(Debug, Clone)]
pub struct DriveSyncAdapter {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    folder_name: String,
    http: Arc<Client>,
    tokens: Arc<Mutex<Option<TokenSet>>>,
    folders: Arc<Mutex<FolderIds>>,
}

impl DriveSyncAdapter {
    /// Build an adapter from a refresh token (already stored
    /// in the OS keychain after the OAuth dance completed)
    /// plus the user's app credentials and chosen folder
    /// name.
    pub fn new(
        config: GoogleDriveAccountConfig,
        refresh_token: impl Into<String>,
    ) -> GoogleDriveResult<Self> {
        if config.client_id.trim().is_empty() {
            return Err(GoogleDriveError::Config(
                "client_id must not be empty".into(),
            ));
        }
        let folder_name = if config.folder_name.trim().is_empty() {
            "Aperio".to_string()
        } else {
            config.folder_name.trim().to_string()
        };
        Ok(Self {
            client_id: config.client_id,
            client_secret: config.client_secret,
            refresh_token: refresh_token.into(),
            folder_name,
            http: Arc::new(Client::new()),
            tokens: Arc::new(Mutex::new(None)),
            folders: Arc::new(Mutex::new(FolderIds::default())),
        })
    }

    /// Borrow a fresh-enough access token. Mirrors the
    /// Dropbox adapter's logic — same 60s safety margin,
    /// same Mutex-based serialisation across concurrent
    /// callers.
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
            .map_err(drive_to_sync)?;
            *guard = Some(refreshed);
        }
        Ok(guard.as_ref().unwrap().access_token.clone())
    }

    async fn force_refresh(&self) -> SyncResult<String> {
        {
            let mut guard = self.tokens.lock().await;
            *guard = None;
        }
        self.access_token().await
    }

    /// Resolve + cache the four folder IDs (base, log,
    /// assets, sounds). Idempotent: subsequent calls
    /// short-circuit once the cache is filled.
    async fn ensure_folder_ids(
        &self,
        access_token: &str,
    ) -> SyncResult<FolderIds> {
        let mut guard = self.folders.lock().await;
        if guard.base.is_some()
            && guard.log.is_some()
            && guard.assets.is_some()
            && guard.sounds.is_some()
        {
            return Ok(guard.clone());
        }
        // Resolve base under "root" (the special id for the
        // user's My Drive root). create_folder is idempotent
        // — it find_childs first and only creates on miss.
        if guard.base.is_none() {
            let id = files::create_folder(
                &self.http,
                access_token,
                "root",
                &self.folder_name,
            )
            .await
            .map_err(drive_to_sync)?;
            guard.base = Some(id);
        }
        let base_id = guard.base.as_ref().unwrap().clone();
        if guard.log.is_none() {
            let id = files::create_folder(&self.http, access_token, &base_id, "log")
                .await
                .map_err(drive_to_sync)?;
            guard.log = Some(id);
        }
        if guard.assets.is_none() {
            let id = files::create_folder(
                &self.http,
                access_token,
                &base_id,
                "assets",
            )
            .await
            .map_err(drive_to_sync)?;
            guard.assets = Some(id);
        }
        let assets_id = guard.assets.as_ref().unwrap().clone();
        if guard.sounds.is_none() {
            let id = files::create_folder(
                &self.http,
                access_token,
                &assets_id,
                "sounds",
            )
            .await
            .map_err(drive_to_sync)?;
            guard.sounds = Some(id);
        }
        Ok(guard.clone())
    }

    /// One-shot 401 retry around `ensure_folder_ids`. If the
    /// token has expired between cache-fetch and use,
    /// refresh + try once more.
    async fn ensure_folder_ids_with_retry(&self) -> SyncResult<FolderIds> {
        let token = self.access_token().await?;
        match self.ensure_folder_ids(&token).await {
            Ok(ids) => Ok(ids),
            Err(err) if matches!(err, SyncError::Auth(_)) => {
                let token = self.force_refresh().await?;
                self.ensure_folder_ids(&token).await
            }
            Err(err) => Err(err),
        }
    }
}

#[async_trait]
impl SyncAdapter for DriveSyncAdapter {
    async fn test_connection(&self) -> SyncResult<()> {
        let token = self.access_token().await?;
        // Cheap aliveness + token probe.
        files::check_user(&self.http, &token)
            .await
            .map_err(drive_to_sync)?;
        // Lazy-create the dataset folders so the first push
        // hits a populated structure.
        self.ensure_folder_ids_with_retry().await?;
        Ok(())
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let base_id = folders.base.unwrap();
        let token = self.access_token().await?;
        let id =
            match files::find_child(&self.http, &token, &base_id, "meta.json")
                .await
            {
                Ok(Some(id)) => id,
                Ok(None) => return Ok(None),
                Err(err) if err.is_auth() => {
                    let token = self.force_refresh().await?;
                    match files::find_child(
                        &self.http,
                        &token,
                        &base_id,
                        "meta.json",
                    )
                    .await
                    {
                        Ok(Some(id)) => id,
                        Ok(None) => return Ok(None),
                        Err(err) => return Err(drive_to_sync(err)),
                    }
                }
                Err(err) => return Err(drive_to_sync(err)),
            };
        let token = self.access_token().await?;
        let bytes = files::download(&self.http, &token, &id)
            .await
            .map_err(drive_to_sync)?;
        match bytes {
            Some(b) => Ok(Some(MetaJson::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let base_id = folders.base.unwrap();
        let bytes = meta.to_bytes()?;
        self.upload_to_parent(&base_id, "meta.json", bytes).await
    }

    async fn fetch_new_logs(
        &self,
        since: &DeviceCursor,
    ) -> SyncResult<Vec<LogFile>> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let log_id = folders.log.unwrap();
        let token = self.access_token().await?;
        let names = match files::list_children(&self.http, &token, &log_id).await
        {
            Ok(n) => n,
            Err(err) if err.is_auth() => {
                let token = self.force_refresh().await?;
                files::list_children(&self.http, &token, &log_id)
                    .await
                    .map_err(drive_to_sync)?
            }
            Err(err) if err.is_not_found() => return Ok(Vec::new()),
            Err(err) => return Err(drive_to_sync(err)),
        };

        let mut wanted: Vec<LogFileName> = Vec::new();
        for name in names {
            let parsed = match LogFileName::from_filename(&name) {
                Ok(p) => p,
                Err(_) => {
                    debug!(name = %name, "skipping non-log entry in list_children");
                    continue;
                }
            };
            if parsed.timestamp > since.last_seen_log {
                wanted.push(parsed);
            }
        }

        let mut out = Vec::with_capacity(wanted.len());
        for parsed in wanted {
            let token = self.access_token().await?;
            let filename = parsed.to_filename();
            let id = match files::find_child(
                &self.http,
                &token,
                &log_id,
                &filename,
            )
            .await
            {
                Ok(Some(id)) => id,
                Ok(None) => {
                    debug!(name = %filename, "log listed but no longer present");
                    continue;
                }
                Err(err) => {
                    warn!(name = %filename, ?err, "find_child for log failed");
                    continue;
                }
            };
            match files::download(&self.http, &token, &id).await {
                Ok(Some(bytes)) => out.push(LogFile {
                    name: parsed,
                    bytes,
                }),
                Ok(None) => debug!(name = %filename, "log gone between list + download"),
                Err(err) => warn!(name = %filename, ?err, "download log failed"),
            }
        }
        Ok(out)
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let log_id = folders.log.unwrap();
        let filename = log.name.to_filename();
        self.upload_to_parent(&log_id, &filename, log.bytes.clone())
            .await
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let base_id = folders.base.unwrap();
        let token = self.access_token().await?;
        let id = match files::find_child(
            &self.http,
            &token,
            &base_id,
            "snapshot.json",
        )
        .await
        {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(None),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh().await?;
                match files::find_child(
                    &self.http,
                    &token,
                    &base_id,
                    "snapshot.json",
                )
                .await
                {
                    Ok(Some(id)) => id,
                    Ok(None) => return Ok(None),
                    Err(err) => return Err(drive_to_sync(err)),
                }
            }
            Err(err) => return Err(drive_to_sync(err)),
        };
        let token = self.access_token().await?;
        let bytes = files::download(&self.http, &token, &id)
            .await
            .map_err(drive_to_sync)?;
        match bytes {
            Some(b) => Ok(Some(Snapshot::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let base_id = folders.base.unwrap();
        let bytes = snapshot.to_bytes()?;
        self.upload_to_parent(&base_id, "snapshot.json", bytes).await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let log_id = folders.log.unwrap();
        let token = self.access_token().await?;
        let filename = name.to_filename();
        let id =
            match files::find_child(&self.http, &token, &log_id, &filename)
                .await
            {
                Ok(Some(id)) => id,
                // "Already gone" is the goal of delete; honour
                // the SFTP / FTP / Dropbox convention.
                Ok(None) => return Ok(()),
                Err(err) if err.is_auth() => {
                    let token = self.force_refresh().await?;
                    match files::find_child(
                        &self.http,
                        &token,
                        &log_id,
                        &filename,
                    )
                    .await
                    {
                        Ok(Some(id)) => id,
                        Ok(None) => return Ok(()),
                        Err(err) => return Err(drive_to_sync(err)),
                    }
                }
                Err(err) if err.is_not_found() => return Ok(()),
                Err(err) => return Err(drive_to_sync(err)),
            };
        let token = self.access_token().await?;
        files::delete(&self.http, &token, &id)
            .await
            .map_err(drive_to_sync)
    }

    async fn push_sound_asset(
        &self,
        hash: &str,
        extension: &str,
        bytes: &[u8],
    ) -> SyncResult<()> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let sounds_id = folders.sounds.unwrap();
        let name = format!("{hash}.{extension}");
        self.upload_to_parent(&sounds_id, &name, bytes.to_vec())
            .await
    }

    async fn fetch_sound_asset(
        &self,
        hash: &str,
        extension: &str,
    ) -> SyncResult<Option<Vec<u8>>> {
        let folders = self.ensure_folder_ids_with_retry().await?;
        let sounds_id = folders.sounds.unwrap();
        let name = format!("{hash}.{extension}");
        let token = self.access_token().await?;
        let id = match files::find_child(&self.http, &token, &sounds_id, &name)
            .await
        {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(None),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh().await?;
                match files::find_child(&self.http, &token, &sounds_id, &name)
                    .await
                {
                    Ok(Some(id)) => id,
                    Ok(None) => return Ok(None),
                    Err(err) => return Err(drive_to_sync(err)),
                }
            }
            Err(err) => return Err(drive_to_sync(err)),
        };
        let token = self.access_token().await?;
        files::download(&self.http, &token, &id)
            .await
            .map_err(drive_to_sync)
    }
}

impl DriveSyncAdapter {
    /// Upload `bytes` as a file named `name` under `parent_id`.
    /// If a file with that name already exists, PATCH its
    /// content; otherwise multipart-create it. One-shot 401
    /// retry on auth failure.
    async fn upload_to_parent(
        &self,
        parent_id: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> SyncResult<()> {
        let token = self.access_token().await?;
        match self
            .upload_inner(&token, parent_id, name, &bytes)
            .await
        {
            Ok(()) => Ok(()),
            Err(err) if matches!(err, SyncError::Auth(_)) => {
                let token = self.force_refresh().await?;
                self.upload_inner(&token, parent_id, name, &bytes).await
            }
            Err(err) => Err(err),
        }
    }

    async fn upload_inner(
        &self,
        token: &str,
        parent_id: &str,
        name: &str,
        bytes: &[u8],
    ) -> SyncResult<()> {
        // Find any existing file with this name to decide
        // between PATCH (update) and POST multipart (create).
        let existing = files::find_child(&self.http, token, parent_id, name)
            .await
            .map_err(drive_to_sync)?;
        let _new_id = files::upload(
            &self.http,
            token,
            parent_id,
            name,
            bytes,
            existing.as_deref(),
        )
        .await
        .map_err(drive_to_sync)?;
        Ok(())
    }
}

/// Map a [`GoogleDriveError`] into the [`SyncError`] vocabulary.
fn drive_to_sync(err: GoogleDriveError) -> SyncError {
    match err {
        GoogleDriveError::Auth(msg) => SyncError::auth(msg),
        GoogleDriveError::NotFound(msg) => SyncError::not_found(msg),
        GoogleDriveError::Config(msg) => SyncError::internal(msg),
        GoogleDriveError::Protocol(msg) => SyncError::protocol(msg),
        GoogleDriveError::Http { status, message } => {
            SyncError::network(format!("HTTP {status}: {message}"))
        }
        GoogleDriveError::Io(msg) => SyncError::io(msg),
        GoogleDriveError::Csrf => {
            SyncError::auth("OAuth state mismatch (CSRF)")
        }
        GoogleDriveError::AuthTimeout => {
            SyncError::auth("OAuth dance timed out")
        }
        GoogleDriveError::AuthDenied(msg) => {
            SyncError::auth(format!("OAuth consent denied: {msg}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_name_defaults_to_aperio_when_empty() {
        let adapter = DriveSyncAdapter::new(
            GoogleDriveAccountConfig {
                client_id: "x".into(),
                client_secret: "y".into(),
                folder_name: "".into(),
            },
            "refresh-token",
        )
        .unwrap();
        assert_eq!(adapter.folder_name, "Aperio");
    }

    #[test]
    fn folder_name_trims_whitespace() {
        let adapter = DriveSyncAdapter::new(
            GoogleDriveAccountConfig {
                client_id: "x".into(),
                client_secret: "y".into(),
                folder_name: "  MyAperio  ".into(),
            },
            "refresh-token",
        )
        .unwrap();
        assert_eq!(adapter.folder_name, "MyAperio");
    }

    #[test]
    fn empty_client_id_rejected() {
        let result = DriveSyncAdapter::new(
            GoogleDriveAccountConfig {
                client_id: "".into(),
                client_secret: "y".into(),
                folder_name: "Aperio".into(),
            },
            "refresh-token",
        );
        assert!(matches!(result, Err(GoogleDriveError::Config(_))));
    }
}
