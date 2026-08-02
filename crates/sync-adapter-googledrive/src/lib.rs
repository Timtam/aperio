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
//! IDs we cache (one `Mutex<RemoteIds>` so concurrent trait
//! methods don't all re-resolve):
//!
//! - `base` — the "aperio" (or whatever the user named it)
//!   folder under the user's Drive root.
//! - `log` — `log/` under base.
//! - `assets` / `sounds` — `assets/` + `assets/sounds/` under
//!   base.
//! - `meta_file` / `snapshot_file` — the two singleton files.
//!   Their IDs are stable for life: updates PATCH the content
//!   under the existing ID and the adapter never renames or
//!   deletes them. A cached ID that 404s (the user cleaned up
//!   the folder remotely) is invalidated and re-resolved once.
//!
//! Folders resolve lazily and PER NEED via `create_folder`
//! (which is idempotent — returns the existing ID when
//! present): meta/snapshot ops resolve only `base`, log ops
//! `base` + `log/`, sound ops the full `assets/sounds/` chain.
//! A round that never touches sounds never pays the sounds
//! chain's find_childs.
//!
//! Log downloads reuse the ID the listing already returned
//! (Drive doesn't reuse IDs across deletes, so a stale cache
//! would only 404). Log PUSHES cache filename → file id after a
//! successful upload: the orchestrator re-pushes the SAME name
//! whenever the live session file grows, and the cached id turns
//! that routine re-push into a single in-place PATCH. A push
//! WITHOUT a cached id always probes first — Drive stores
//! duplicate names happily, and a create whose response was lost
//! (or that predates an app restart, which wipes the in-memory
//! cache) is only detectable by asking the server. The remaining
//! per-op `find_child` cases (delete_log, sound assets) are cold
//! paths where an ID cache buys nothing.
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

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sync_core::{
    DeviceCursor, LogFile, LogFileName, MetaJson, Snapshot, SyncAdapter, SyncError, SyncResult,
};
use tokio::sync::Mutex;
use tracing::debug;

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

/// Resolved remote IDs cached for the adapter's lifetime: the
/// folder chain plus the two singleton files (see the module
/// docs for why those two are safe to cache). Folder slots are
/// filled lazily by [`DriveSyncAdapter::ensure_folders`].
#[derive(Debug, Default, Clone)]
struct RemoteIds {
    base: Option<String>,
    log: Option<String>,
    assets: Option<String>,
    sounds: Option<String>,
    meta_file: Option<String>,
    snapshot_file: Option<String>,
}

/// How much of the folder chain an operation needs resolved.
/// Keeps cold-path round-trips proportional to the operation:
/// meta/snapshot ops touch only `base`, log ops `base` + `log/`,
/// sound ops the full `assets/sounds/` chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FolderNeed {
    Base,
    Log,
    Sounds,
}

/// The two singleton files under `base` whose IDs we cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Singleton {
    Meta,
    Snapshot,
}

impl Singleton {
    fn filename(self) -> &'static str {
        match self {
            Singleton::Meta => "meta.json",
            Singleton::Snapshot => "snapshot.json",
        }
    }
}

/// Google Drive `SyncAdapter`. Holds the app credentials +
/// refresh token + the lazily-resolved remote IDs. Cheap to
/// clone — inner state is Arc-shared.
#[derive(Debug, Clone)]
pub struct DriveSyncAdapter {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    folder_name: String,
    http: Arc<Client>,
    tokens: Arc<Mutex<Option<TokenSet>>>,
    ids: Arc<Mutex<RemoteIds>>,
    /// File ids of log files successfully pushed this session,
    /// keyed by filename. The orchestrator re-pushes the SAME
    /// filename whenever the live session file grows (and once
    /// more per app launch), so a cached id turns that routine
    /// re-push into one duplicate-proof in-place PATCH. Without a
    /// cached id, push_log always probes: Drive allows duplicate
    /// names, so a create whose response was lost — or one from
    /// before a restart wiped this map — can only be detected by
    /// asking the server. std Mutex — never held across an await.
    log_file_ids: Arc<StdMutex<HashMap<String, String>>>,
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
            ids: Arc::new(Mutex::new(RemoteIds::default())),
            log_file_ids: Arc::new(StdMutex::new(HashMap::new())),
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

    /// Resolve + cache the folder IDs an operation needs (see
    /// [`FolderNeed`]). Idempotent: filled slots short-circuit,
    /// so a warm adapter pays zero requests here.
    async fn ensure_folders(&self, access_token: &str, need: FolderNeed) -> SyncResult<RemoteIds> {
        let mut guard = self.ids.lock().await;
        // Resolve base under "root" (the special id for the
        // user's My Drive root). create_folder is idempotent
        // — it find_childs first and only creates on miss.
        if guard.base.is_none() {
            let id = files::create_folder(&self.http, access_token, "root", &self.folder_name)
                .await
                .map_err(drive_to_sync)?;
            guard.base = Some(id);
        }
        let base_id = guard.base.as_ref().unwrap().clone();
        if need == FolderNeed::Log && guard.log.is_none() {
            let id = files::create_folder(&self.http, access_token, &base_id, "log")
                .await
                .map_err(drive_to_sync)?;
            guard.log = Some(id);
        }
        if need == FolderNeed::Sounds {
            if guard.assets.is_none() {
                let id = files::create_folder(&self.http, access_token, &base_id, "assets")
                    .await
                    .map_err(drive_to_sync)?;
                guard.assets = Some(id);
            }
            let assets_id = guard.assets.as_ref().unwrap().clone();
            if guard.sounds.is_none() {
                let id = files::create_folder(&self.http, access_token, &assets_id, "sounds")
                    .await
                    .map_err(drive_to_sync)?;
                guard.sounds = Some(id);
            }
        }
        Ok(guard.clone())
    }

    /// One-shot 401 retry around `ensure_folders`. If the
    /// token has expired between cache-fetch and use,
    /// refresh + try once more.
    async fn folders_for(&self, need: FolderNeed) -> SyncResult<RemoteIds> {
        let token = self.access_token().await?;
        match self.ensure_folders(&token, need).await {
            Ok(ids) => Ok(ids),
            Err(SyncError::Auth(_)) => {
                let token = self.force_refresh().await?;
                self.ensure_folders(&token, need).await
            }
            Err(err) => Err(err),
        }
    }

    async fn cached_singleton_id(&self, which: Singleton) -> Option<String> {
        let guard = self.ids.lock().await;
        match which {
            Singleton::Meta => guard.meta_file.clone(),
            Singleton::Snapshot => guard.snapshot_file.clone(),
        }
    }

    async fn store_singleton_id(&self, which: Singleton, id: Option<String>) {
        let mut guard = self.ids.lock().await;
        let slot = match which {
            Singleton::Meta => &mut guard.meta_file,
            Singleton::Snapshot => &mut guard.snapshot_file,
        };
        *slot = id;
    }

    /// Fetch a singleton file's bytes, going through the ID cache:
    /// a warm hit is a single GET. A cached ID that 404s is
    /// invalidated and re-resolved exactly once (fresh find_child);
    /// `Ok(None)` when the file genuinely doesn't exist. One-shot
    /// 401 refresh-retry around the whole flow.
    async fn fetch_singleton(
        &self,
        base_id: &str,
        which: Singleton,
    ) -> SyncResult<Option<Vec<u8>>> {
        let token = self.access_token().await?;
        match self.fetch_singleton_inner(&token, base_id, which).await {
            Err(SyncError::Auth(_)) => {
                let token = self.force_refresh().await?;
                self.fetch_singleton_inner(&token, base_id, which).await
            }
            other => other,
        }
    }

    async fn fetch_singleton_inner(
        &self,
        token: &str,
        base_id: &str,
        which: Singleton,
    ) -> SyncResult<Option<Vec<u8>>> {
        if let Some(id) = self.cached_singleton_id(which).await {
            match files::download(&self.http, token, &id).await {
                Ok(Some(bytes)) => return Ok(Some(bytes)),
                Ok(None) => {
                    // Cached ID went stale (the file was deleted
                    // remotely, e.g. user cleanup). Drop it and fall
                    // through to one fresh resolution below.
                    self.store_singleton_id(which, None).await;
                }
                Err(err) => return Err(drive_to_sync(err)),
            }
        }
        let id = match files::find_child(&self.http, token, base_id, which.filename())
            .await
            .map_err(drive_to_sync)?
        {
            Some(id) => id,
            None => return Ok(None),
        };
        self.store_singleton_id(which, Some(id.clone())).await;
        match files::download(&self.http, token, &id)
            .await
            .map_err(drive_to_sync)?
        {
            Some(bytes) => Ok(Some(bytes)),
            None => {
                // Freshly-resolved ID gone between find + GET: treat
                // as absent, don't loop — and don't keep the dead ID.
                self.store_singleton_id(which, None).await;
                Ok(None)
            }
        }
    }

    /// Push a singleton file through the ID cache: a warm hit is a
    /// single PATCH. A stale cached ID (404 on PATCH) is invalidated
    /// and re-resolved exactly once; the resulting ID — PATCHed or
    /// freshly created — is cached for the next round. One-shot 401
    /// refresh-retry around the whole flow.
    async fn push_singleton(
        &self,
        base_id: &str,
        which: Singleton,
        bytes: &[u8],
    ) -> SyncResult<()> {
        let token = self.access_token().await?;
        match self
            .push_singleton_inner(&token, base_id, which, bytes)
            .await
        {
            Err(SyncError::Auth(_)) => {
                let token = self.force_refresh().await?;
                self.push_singleton_inner(&token, base_id, which, bytes)
                    .await
            }
            other => other,
        }
    }

    async fn push_singleton_inner(
        &self,
        token: &str,
        base_id: &str,
        which: Singleton,
        bytes: &[u8],
    ) -> SyncResult<()> {
        if let Some(id) = self.cached_singleton_id(which).await {
            match files::upload(
                &self.http,
                token,
                base_id,
                which.filename(),
                bytes,
                Some(&id),
            )
            .await
            {
                Ok(_) => return Ok(()),
                Err(err) if err.is_not_found() => {
                    // Stale cached ID — invalidate and fall through to
                    // the probe-then-upload path below.
                    self.store_singleton_id(which, None).await;
                }
                Err(err) => return Err(drive_to_sync(err)),
            }
        }
        let existing = files::find_child(&self.http, token, base_id, which.filename())
            .await
            .map_err(drive_to_sync)?;
        let id = files::upload(
            &self.http,
            token,
            base_id,
            which.filename(),
            bytes,
            existing.as_deref(),
        )
        .await
        .map_err(drive_to_sync)?;
        self.store_singleton_id(which, Some(id)).await;
        Ok(())
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
        // Warm the FULL folder structure so the first real push
        // after setup hits populated folders (per-need resolution
        // elsewhere only builds what an operation touches).
        self.folders_for(FolderNeed::Log).await?;
        self.folders_for(FolderNeed::Sounds).await?;
        Ok(())
    }

    async fn fetch_meta(&self) -> SyncResult<Option<MetaJson>> {
        let ids = self.folders_for(FolderNeed::Base).await?;
        let base_id = ids.base.unwrap();
        match self.fetch_singleton(&base_id, Singleton::Meta).await? {
            Some(b) => Ok(Some(MetaJson::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_meta(&self, meta: &MetaJson) -> SyncResult<()> {
        let ids = self.folders_for(FolderNeed::Base).await?;
        let base_id = ids.base.unwrap();
        let bytes = meta.to_bytes()?;
        self.push_singleton(&base_id, Singleton::Meta, &bytes).await
    }

    async fn fetch_new_logs(&self, since: &DeviceCursor) -> SyncResult<Vec<LogFile>> {
        let ids = self.folders_for(FolderNeed::Log).await?;
        let log_id = ids.log.unwrap();
        let token = self.access_token().await?;
        let entries = match files::list_children(&self.http, &token, &log_id).await {
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

        let wanted = select_wanted_logs(since, entries);

        // Download by the LISTED ids with bounded concurrency; the
        // token is resolved once for the whole batch, with the
        // one-shot 401 refresh-retry wrapped around the batch rather
        // than per file. A retried batch re-downloads at most the
        // handful of files the failed attempt already got — fine for
        // the rare token-expiry race.
        let token = self.access_token().await?;
        match self.download_logs(&token, wanted.clone()).await {
            Err(SyncError::Auth(_)) => {
                let token = self.force_refresh().await?;
                self.download_logs(&token, wanted).await
            }
            other => other,
        }
    }

    async fn push_log(&self, log: &LogFile) -> SyncResult<()> {
        let ids = self.folders_for(FolderNeed::Log).await?;
        let log_id = ids.log.unwrap();
        let filename = log.name.to_filename();
        // Growth re-push fast path: a file we already pushed this
        // session PATCHes in place under its cached id — one
        // request, and an in-place overwrite by construction. A
        // stale id (the compactor deleted the file behind us) is
        // evicted and falls through to the probing path below,
        // mirroring push_singleton_inner's stale-id handling.
        let cached_id = self.log_file_ids.lock().unwrap().get(&filename).cloned();
        if let Some(id) = cached_id {
            if self
                .patch_in_place(&log_id, &filename, &log.bytes, &id)
                .await?
            {
                return Ok(());
            }
            self.log_file_ids.lock().unwrap().remove(&filename);
        }
        // No usable cached id → probe-then-upload. Probing by
        // default is what keeps this idempotent across restarts: an
        // earlier create may have landed server-side even though its
        // response was lost (and Drive allows duplicate names), so
        // only the probe can tell create apart from overwrite. The
        // returned id is cached so the next growth re-push of this
        // file costs a single PATCH.
        let id = self
            .upload_to_parent(&log_id, &filename, log.bytes.clone())
            .await?;
        self.log_file_ids.lock().unwrap().insert(filename, id);
        Ok(())
    }

    async fn fetch_snapshot(&self) -> SyncResult<Option<Snapshot>> {
        let ids = self.folders_for(FolderNeed::Base).await?;
        let base_id = ids.base.unwrap();
        match self.fetch_singleton(&base_id, Singleton::Snapshot).await? {
            Some(b) => Ok(Some(Snapshot::from_bytes(&b)?)),
            None => Ok(None),
        }
    }

    async fn push_snapshot(&self, snapshot: &Snapshot) -> SyncResult<()> {
        let ids = self.folders_for(FolderNeed::Base).await?;
        let base_id = ids.base.unwrap();
        let bytes = snapshot.to_bytes()?;
        self.push_singleton(&base_id, Singleton::Snapshot, &bytes)
            .await
    }

    async fn delete_log(&self, name: &LogFileName) -> SyncResult<()> {
        let ids = self.folders_for(FolderNeed::Log).await?;
        let log_id = ids.log.unwrap();
        let token = self.access_token().await?;
        let filename = name.to_filename();
        // The push-path id cache must not outlive the file: a
        // retired log deleted here would leave a stale id behind
        // (harmless — a PATCH on it 404s into the probe path — but
        // eviction keeps the cache honest).
        self.log_file_ids.lock().unwrap().remove(&filename);
        let id = match files::find_child(&self.http, &token, &log_id, &filename).await {
            Ok(Some(id)) => id,
            // "Already gone" is the goal of delete; honour
            // the SFTP / FTP / Dropbox convention.
            Ok(None) => return Ok(()),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh().await?;
                match files::find_child(&self.http, &token, &log_id, &filename).await {
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

    async fn push_sound_asset(&self, hash: &str, extension: &str, bytes: &[u8]) -> SyncResult<()> {
        let ids = self.folders_for(FolderNeed::Sounds).await?;
        let sounds_id = ids.sounds.unwrap();
        let name = format!("{hash}.{extension}");
        // Sound assets keep the existence probe: names are
        // content-addressed and Drive allows duplicate names, so a
        // probe-less re-push of an already-present asset would
        // create a duplicate instead of being the no-op the trait
        // requires. The returned id is dropped — assets are
        // immutable, so there is no re-push to PATCH.
        self.upload_to_parent(&sounds_id, &name, bytes.to_vec())
            .await
            .map(|_| ())
    }

    async fn fetch_sound_asset(&self, hash: &str, extension: &str) -> SyncResult<Option<Vec<u8>>> {
        let ids = self.folders_for(FolderNeed::Sounds).await?;
        let sounds_id = ids.sounds.unwrap();
        let name = format!("{hash}.{extension}");
        let token = self.access_token().await?;
        let id = match files::find_child(&self.http, &token, &sounds_id, &name).await {
            Ok(Some(id)) => id,
            Ok(None) => return Ok(None),
            Err(err) if err.is_auth() => {
                let token = self.force_refresh().await?;
                match files::find_child(&self.http, &token, &sounds_id, &name).await {
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
    /// content; otherwise multipart-create it. Returns the
    /// resulting file id so callers can cache it. One-shot 401
    /// retry on auth failure.
    async fn upload_to_parent(
        &self,
        parent_id: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> SyncResult<String> {
        let token = self.access_token().await?;
        match self.upload_inner(&token, parent_id, name, &bytes).await {
            Ok(id) => Ok(id),
            Err(SyncError::Auth(_)) => {
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
    ) -> SyncResult<String> {
        // Find any existing file with this name to decide
        // between PATCH (update) and POST multipart (create).
        let existing = files::find_child(&self.http, token, parent_id, name)
            .await
            .map_err(drive_to_sync)?;
        files::upload(
            &self.http,
            token,
            parent_id,
            name,
            bytes,
            existing.as_deref(),
        )
        .await
        .map_err(drive_to_sync)
    }

    /// PATCH content under a known `file_id` with the usual
    /// one-shot 401 retry. `Ok(true)` = patched in place;
    /// `Ok(false)` = the id no longer exists (the compactor
    /// deleted the file behind us) — the caller should drop its
    /// cached id and fall back to the probing path.
    async fn patch_in_place(
        &self,
        parent_id: &str,
        name: &str,
        bytes: &[u8],
        file_id: &str,
    ) -> SyncResult<bool> {
        let token = self.access_token().await?;
        let result =
            match files::upload(&self.http, &token, parent_id, name, bytes, Some(file_id)).await {
                Err(err) if err.is_auth() => {
                    let token = self.force_refresh().await?;
                    files::upload(&self.http, &token, parent_id, name, bytes, Some(file_id)).await
                }
                other => other,
            };
        match result {
            Ok(_) => Ok(true),
            Err(err) if err.is_not_found() => Ok(false),
            Err(err) => Err(drive_to_sync(err)),
        }
    }

    /// Download the selected logs by their listed IDs with bounded
    /// concurrency — same shape as WebDAV's parallel GET batch. The
    /// orchestrator sorts chronologically before apply, so unordered
    /// completion is fine. Error semantics per
    /// [`dispose_log_download`]: one silent-skip case, everything
    /// else fails the whole fetch.
    async fn download_logs(
        &self,
        token: &str,
        wanted: Vec<(LogFileName, String)>,
    ) -> SyncResult<Vec<LogFile>> {
        use futures::stream::{self, StreamExt};
        // Modest bound: googleapis.com is not shy at 4, and the
        // reqwest pool reuses the connections.
        const LOG_FETCH_CONCURRENCY: usize = 4;
        let results: Vec<SyncResult<Option<LogFile>>> = stream::iter(wanted)
            .map(|(name, id)| async move {
                dispose_log_download(&name, files::download(&self.http, token, &id).await)
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
}

/// Filter a log-folder listing down to the (parsed name, file id)
/// pairs the cursor wants. Pure — extracted for unit testing.
///
/// `wants_sized` closes the append-miss class: a peer's live session
/// file that gained appended events since we applied it is re-fetched
/// even though its timestamp sits at/below the cursor. Drive's listed
/// `size` is the raw stored byte count (ciphertext under E2E), which
/// is exactly the domain the encrypting layer translates the cursor's
/// known_lengths into — pass it through unadjusted.
fn select_wanted_logs(
    since: &DeviceCursor,
    entries: Vec<files::DriveEntry>,
) -> Vec<(LogFileName, String)> {
    let mut wanted = Vec::new();
    for entry in entries {
        let parsed = match LogFileName::from_filename(&entry.name) {
            Ok(p) => p,
            Err(_) => {
                debug!(name = %entry.name, "skipping non-log entry in list_children");
                continue;
            }
        };
        if since.wants_sized(&parsed, &entry.name, entry.size) {
            wanted.push((parsed, entry.id));
        }
    }
    wanted
}

/// Per-file disposition for a batched log download. Exactly one
/// silent-skip case survives: `Ok(None)` — the file was listed but
/// deleted (compactor) between list and GET; the next round sees a
/// fresh listing. Every other failure fails the WHOLE fetch: the
/// orchestrator advances the cursor from the returned logs only, so
/// silently skipping a failed file would strand it below the cursor
/// with no applied-length record — its events lost on this device
/// forever. Failing the fetch serves stale and retries next round.
fn dispose_log_download(
    name: &LogFileName,
    result: GoogleDriveResult<Option<Vec<u8>>>,
) -> SyncResult<Option<LogFile>> {
    match result {
        Ok(Some(bytes)) => Ok(Some(LogFile {
            name: name.clone(),
            bytes,
        })),
        Ok(None) => {
            debug!(name = %name.to_filename(), "log listed but gone before download");
            Ok(None)
        }
        Err(err) => Err(drive_to_sync(err)),
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
        GoogleDriveError::Csrf => SyncError::auth("OAuth state mismatch (CSRF)"),
        GoogleDriveError::AuthTimeout => SyncError::auth("OAuth dance timed out"),
        GoogleDriveError::AuthDenied(msg) => {
            SyncError::auth(format!("OAuth consent denied: {msg}"))
        }
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

    fn entry(name: &str, id: &str, size: Option<u64>) -> files::DriveEntry {
        files::DriveEntry {
            id: id.into(),
            name: name.into(),
            size,
        }
    }

    /// Drive twin of cal-adapter-local's
    /// `grown_file_at_the_cursor_is_refetched`: the listed size
    /// feeding `wants_sized` closes the append-miss class.
    #[test]
    fn grown_file_at_the_cursor_is_selected_for_refetch() {
        let file = log_name(1_000, "peer");
        let filename = file.to_filename();
        let cursor = DeviceCursor {
            // Cursor AT the file's timestamp — the plain filter
            // would skip it.
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: vec![KnownLogLength {
                name: filename.clone(),
                len: 100,
            }],
        };
        // Grown past the applied length → selected, with the listed
        // id carried through for the direct download.
        let wanted = select_wanted_logs(&cursor, vec![entry(&filename, "id-grown", Some(150))]);
        assert_eq!(wanted, vec![(file, "id-grown".to_string())]);
        // Unchanged → skipped.
        assert!(
            select_wanted_logs(&cursor, vec![entry(&filename, "id-same", Some(100))]).is_empty()
        );
        // No listed size (Docs-native oddity) → plain cursor
        // semantics, skipped.
        assert!(select_wanted_logs(&cursor, vec![entry(&filename, "id-nosize", None)]).is_empty());
    }

    #[test]
    fn selection_applies_cursor_exclusion_and_non_log_filtering() {
        let cursor = DeviceCursor {
            last_seen_log: Utc.timestamp_opt(1_000, 0).unwrap(),
            exclude_device: Some(DeviceId::from_string("me".into())),
            known_lengths: Vec::new(),
        };
        let newer = log_name(2_000, "peer");
        let entries = vec![
            entry(&newer.to_filename(), "id-new", Some(10)),
            // Own device → excluded even though newer.
            entry(&log_name(2_000, "me").to_filename(), "id-own", Some(10)),
            // Below the horizon, no growth record → skipped.
            entry(&log_name(500, "peer").to_filename(), "id-old", Some(10)),
            // Not a log filename → skipped.
            entry("stray-notes.txt", "id-junk", None),
        ];
        let wanted = select_wanted_logs(&cursor, entries);
        assert_eq!(wanted, vec![(newer, "id-new".to_string())]);
    }

    #[test]
    fn download_disposition_skips_only_the_listed_but_gone_race() {
        let name = log_name(2_000, "peer");
        // Bytes arrived → kept.
        let kept = dispose_log_download(&name, Ok(Some(vec![1, 2, 3]))).unwrap();
        assert_eq!(kept.map(|l| l.bytes), Some(vec![1, 2, 3]));
        // Listed but 404 on download (compactor deleted it between
        // list and GET) → the single silent skip.
        assert!(dispose_log_download(&name, Ok(None)).unwrap().is_none());
        // Any real failure fails the whole fetch so the cursor can
        // never advance past a file the adapter withheld.
        assert!(dispose_log_download(
            &name,
            Err(GoogleDriveError::Http {
                status: 500,
                message: "backend error".into(),
            }),
        )
        .is_err());
    }

    #[test]
    fn log_file_id_cache_is_shared_across_clones() {
        // The duplicate-proofing depends on ONE id cache per
        // adapter: the host hands out clones, and a per-clone map
        // would silently regress the growth re-push to a probe per
        // clone — or worse, let two clones race probe-and-create.
        let adapter = DriveSyncAdapter::new(
            GoogleDriveAccountConfig {
                client_id: "x".into(),
                client_secret: "y".into(),
                folder_name: "Aperio".into(),
            },
            "refresh-token",
        )
        .unwrap();
        let clone = adapter.clone();
        adapter
            .log_file_ids
            .lock()
            .unwrap()
            .insert("f.jsonl".into(), "id-1".into());
        assert_eq!(
            clone.log_file_ids.lock().unwrap().get("f.jsonl"),
            Some(&"id-1".to_string())
        );
    }

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
