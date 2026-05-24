//! Cross-device sync commands (DESIGN.md §19, Phases Sd–Sf).
//!
//! Verbs exposed to the frontend:
//!
//!   - `configure_sync_adapter(config)` — install / swap the
//!     runtime adapter and persist the choice in `user_prefs`. The
//!     "steady state" command, used once the user has already
//!     decided which dataset they're joining.
//!   - `sync_now()` — manual trigger. Returns a `SyncRoundReport`
//!     so the dialogue can show "12 events applied" without a
//!     follow-up status fetch.
//!   - `get_sync_status()` — read-only snapshot for the status
//!     indicator.
//!   - `set_sync_interval(minutes)` — Phase Se. Adjusts the
//!     periodic scheduler.
//!   - `preview_sync_target(config)` — Phase Sf. Reads `meta.json`
//!     at the given config WITHOUT touching the live orchestrator.
//!     Frontend uses the result to drive the onboarding dialog.
//!   - `accept_remote_dataset(config, deviceName)` — Phase Sf.
//!     "Datensatz übernehmen". Configures the adapter, pulls every
//!     log, applies, registers this device in meta.json.
//!   - `adopt_local_dataset(config, deviceName)` — Phase Sf.
//!     "Neu beginnen". Overwrites the remote `meta.json` with one
//!     that names only this device. The frontend is responsible for
//!     the destructive-action confirmation prompt.
//!
//! Adapter configuration values DO NOT propagate via the event log
//! (per §19.2.1) — they're device-local. The user_prefs whitelist
//! already excludes everything under `sync.adapter.*`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use sync_adapter_local::LocalFsSyncAdapter;
use sync_adapter_webdav::{WebDavCredentials, WebDavSyncAdapter};
use sync_core::SyncAdapter;
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::{
    CompactionReport, OnboardingReport, OnboardingService, SyncOrchestrator, SyncPreview,
    SyncRoundReport, SyncScheduler, SyncStatus,
};
use crate::secrets::{self, SecretSlot};
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key naming the currently-configured adapter
/// family. Empty / missing → no adapter, sync disabled.
const PREF_ADAPTER_KIND: &str = "sync.adapter.kind";

/// Family-specific config (per-kind sub-key). For `kind="local"`
/// we just need a filesystem path.
const PREF_LOCAL_PATH: &str = "sync.adapter.local.path";

/// WebDAV adapter config keys. The URL + user live in user_prefs
/// (device-local; never propagated); the password is stored in the
/// platform keychain via the `secrets` module against a fixed
/// pseudo-account id so we get a single managed slot.
const PREF_WEBDAV_URL: &str = "sync.adapter.webdav.url";
const PREF_WEBDAV_USER: &str = "sync.adapter.webdav.user";

/// Pseudo-account id used to store the WebDAV password in the
/// platform keychain. The `secrets` module is account-scoped; we
/// use this fixed string so the sync adapter has its own managed
/// keychain entry independent of any user-facing account row.
const WEBDAV_SECRET_ACCOUNT: &str = "sync.adapter.webdav";

/// Request body for [`configure_sync_adapter`] and the onboarding
/// commands. The kind is flattened so the frontend can build:
///
/// ```jsonc
/// { "kind": "local",  "path":   "/mnt/nas/aperio" }
/// { "kind": "webdav", "url":    "https://cloud.example.com/.../aperio/",
///                     "user":   "alice",
///                     "password": "hunter2" }    // optional on re-edit
/// { "kind": "none" }   // disconnects any configured adapter
/// ```
///
/// Future adapter kinds (`sftp`, `dropbox`, `googledrive`, …) will
/// add their own branches as new struct variants.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncAdapterConfig {
    /// Filesystem path-based adapter — DESIGN.md §19.6 entry.
    Local { path: String },
    /// WebDAV adapter (DESIGN.md §19.6). The URL must point at the
    /// collection that holds `log/`, `snapshot.json`, etc. — for
    /// Nextcloud that's typically
    /// `https://<host>/remote.php/dav/files/<user>/<folder>/`.
    ///
    /// `password` is optional: if `None`, the previously-stored
    /// keychain password is reused. The Settings UI uses that to
    /// support "edit URL without re-typing the password". An empty
    /// string is treated as "no auth", same as omitting the field.
    Webdav {
        url: String,
        user: String,
        #[serde(default)]
        password: Option<String>,
    },
    /// Explicit disconnect. The orchestrator drops its adapter
    /// handle; subsequent `sync_now` calls return a clear "not
    /// configured" error rather than silently no-oping.
    None,
}

/// Build a fresh adapter instance from a [`SyncAdapterConfig`] —
/// validates the inputs, returns an `Arc<dyn SyncAdapter>` ready to
/// hand to the orchestrator or the onboarding service.
///
/// The `None` variant returns Err: the caller is asking for an
/// adapter to operate on, and a disconnect has no adapter to make.
fn build_adapter(
    config: &SyncAdapterConfig,
) -> CommandResult<Arc<dyn SyncAdapter>> {
    match config {
        SyncAdapterConfig::Local { path } => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "sync path must not be empty".into(),
                });
            }
            Ok(Arc::new(LocalFsSyncAdapter::new(PathBuf::from(trimmed))))
        }
        SyncAdapterConfig::Webdav { url, user, password } => {
            let trimmed_url = url.trim();
            if trimmed_url.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "WebDAV URL must not be empty".into(),
                });
            }
            let trimmed_user = user.trim();
            // Resolve the password from one of two sources:
            //   - the request body (set on a fresh connect / when
            //     the user re-types it in Settings)
            //   - the keychain (set on a URL-only edit, or on app
            //     start when restoring from prefs)
            let resolved_password = match password.as_deref().map(str::trim) {
                Some(p) if !p.is_empty() => Some(p.to_string()),
                _ => secrets::retrieve(WEBDAV_SECRET_ACCOUNT, SecretSlot::Password)
                    .ok(),
            };
            let credentials = match (trimmed_user.is_empty(), resolved_password) {
                (false, Some(pw)) => WebDavCredentials::basic(trimmed_user, &pw),
                _ => WebDavCredentials::None,
            };
            let adapter = WebDavSyncAdapter::new(trimmed_url, credentials)
                .map_err(sync_err)?;
            Ok(Arc::new(adapter))
        }
        SyncAdapterConfig::None => Err(CommandError {
            code: "invalid_input",
            message: "cannot build adapter from None kind".into(),
        }),
    }
}

/// Persist the (already-validated) adapter config into `user_prefs`
/// so the next app start restores the same adapter. Mirrors what
/// `build_adapter_from_prefs` will read back.
fn persist_adapter_config(
    prefs: &UserPrefsRepo,
    config: &SyncAdapterConfig,
) -> CommandResult<()> {
    match config {
        SyncAdapterConfig::Local { path } => {
            let trimmed = path.trim();
            prefs.set(PREF_ADAPTER_KIND, "local").map_err(internal)?;
            prefs.set(PREF_LOCAL_PATH, trimmed).map_err(internal)?;
            Ok(())
        }
        SyncAdapterConfig::Webdav { url, user, password } => {
            prefs.set(PREF_ADAPTER_KIND, "webdav").map_err(internal)?;
            prefs.set(PREF_WEBDAV_URL, url.trim()).map_err(internal)?;
            prefs.set(PREF_WEBDAV_USER, user.trim()).map_err(internal)?;
            // Only overwrite the keychain when the request body
            // explicitly carries a non-empty password. URL/user
            // edits that omit the password keep the prior secret.
            if let Some(pw) = password.as_deref().map(str::trim) {
                if !pw.is_empty() {
                    secrets::store(
                        WEBDAV_SECRET_ACCOUNT,
                        SecretSlot::Password,
                        pw,
                    )
                    .map_err(|err| CommandError {
                        code: "internal",
                        message: format!("keychain store: {err}"),
                    })?;
                }
            }
            Ok(())
        }
        SyncAdapterConfig::None => {
            prefs.delete(PREF_ADAPTER_KIND).map_err(internal)?;
            Ok(())
        }
    }
}

/// Translate a [`sync_core::SyncError`] into a [`CommandError`]
/// with a stable code the frontend can pattern-match.
///
/// Critical mapping for Phase Sf: `NotFound` becomes `not_found`,
/// which the onboarding dialog uses to decide whether to flip from
/// "übernehmen" to "neu beginnen" without surfacing a stack trace.
fn sync_err(err: sync_core::SyncError) -> CommandError {
    use sync_core::SyncError as E;
    let code: &'static str = match &err {
        E::Io(_) => "io",
        E::Network(_) => "network",
        E::Auth(_) => "auth",
        E::Protocol(_) => "protocol",
        E::EncryptionRequired => "encryption_required",
        E::NotFound(_) => "not_found",
        E::SchemaTooOld { .. } => "schema_too_old",
        E::Internal(_) => "internal",
    };
    CommandError {
        code,
        message: err.to_string(),
    }
}

fn internal(err: impl std::fmt::Display) -> CommandError {
    CommandError {
        code: "internal",
        message: err.to_string(),
    }
}

/// Install / swap the active sync adapter. Persists the user's
/// choice so the next app start reconstructs the same adapter
/// in `lib.rs`'s setup phase.
///
/// **Note:** This is the "I already onboarded; just swap the
/// adapter" command. New users go through `preview_sync_target` +
/// `accept_remote_dataset` / `adopt_local_dataset` instead, which
/// configures the adapter as part of the onboarding flow.
#[tauri::command]
pub async fn configure_sync_adapter(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    config: SyncAdapterConfig,
) -> CommandResult<()> {
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    match &config {
        SyncAdapterConfig::Local { .. } | SyncAdapterConfig::Webdav { .. } => {
            // Persist BEFORE building the adapter so the keychain
            // entry for the new WebDAV password is in place; the
            // adapter constructor then reads it back when the
            // request body omitted the password (e.g. URL-only
            // edit). Then we probe.
            persist_adapter_config(&prefs, &config)?;
            let adapter = build_adapter(&config)?;
            // Probe the connection before keeping the adapter
            // active — misconfigurations should surface immediately
            // at the settings dialog, not hours later when the
            // first sync_now runs.
            adapter.test_connection().await.map_err(sync_err)?;
            orchestrator.configure(adapter);
            // Kick the scheduler so the user sees data flow
            // immediately instead of waiting up to one interval
            // for the periodic loop. The debounce window swallows
            // any pile of mutations the writer queued while the
            // adapter was unconfigured.
            scheduler.kick();
        }
        SyncAdapterConfig::None => {
            orchestrator.deconfigure();
            persist_adapter_config(&prefs, &config)?;
            // Keep PREF_LOCAL_PATH / PREF_WEBDAV_* around so re-
            // enabling the same backend is one click away. The
            // keychain password also stays — it's never synced and
            // a user reconnecting to the same dataset wouldn't
            // want to re-type it.
        }
    }
    Ok(())
}

/// Set the periodic sync interval (in minutes). Values below 1 are
/// clamped to 1 so a typo can't pin the scheduler into a hot loop.
/// Returns the value actually persisted so the Settings UI can echo
/// it back into its slider.
#[tauri::command]
pub async fn set_sync_interval(
    scheduler: State<'_, Arc<SyncScheduler>>,
    minutes: u32,
) -> CommandResult<u32> {
    scheduler.set_interval_minutes(minutes).map_err(|err| CommandError {
        code: "internal",
        message: err,
    })
}

/// Trigger one sync round (push pending logs + fetch & apply
/// new ones).
#[tauri::command]
pub async fn sync_now(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
) -> CommandResult<SyncRoundReport> {
    orchestrator.sync_now().await.map_err(sync_err)
}

/// Read-only status snapshot for the status indicator.
#[tauri::command]
pub async fn get_sync_status(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
) -> CommandResult<SyncStatus> {
    Ok(orchestrator.status())
}

/// Manually trigger a compaction round (Phase Sg, §19.10). Snapshots
/// the current local state, pushes `snapshot.json`, advances
/// `meta.json.snapshot_timestamp`, and GCs every log file older than
/// the new snapshot horizon (clamped by the slowest device's
/// `last_seen_log`).
///
/// Returns counters the Settings UI can render directly. The
/// scheduler also dispatches this same code path automatically when
/// the thresholds (§19.10) fire — the command is the "user got
/// impatient" override.
#[tauri::command]
pub async fn compact_now(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
) -> CommandResult<CompactionReport> {
    // Borrow the adapter handle for the duration of the compaction.
    // If none is configured, fail fast with a clear code rather than
    // letting the compactor panic later.
    let adapter = {
        let inner = orchestrator.adapter_handle();
        match inner {
            Some(a) => a,
            None => {
                return Err(CommandError {
                    code: "not_configured",
                    message: "no sync adapter configured".into(),
                });
            }
        }
    };
    orchestrator
        .compactor()
        .compact_now(adapter.as_ref())
        .await
        .map_err(sync_err)
}

// ---------------------------------------------------------------------------
// Phase Sf — onboarding commands.
// ---------------------------------------------------------------------------

/// Probe a remote without committing to it. Reads `meta.json` and
/// returns a [`SyncPreview`] the frontend uses to render the
/// onboarding dialog ("found existing data — adopt or overwrite?").
///
/// Side-effect free: the user can step back from the dialog and
/// pick a different path without leaving cruft in `user_prefs` or
/// the orchestrator state.
#[tauri::command]
pub async fn preview_sync_target(
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
) -> CommandResult<SyncPreview> {
    let adapter = build_adapter(&config)?;
    onboarding.preview(adapter.as_ref()).await.map_err(sync_err)
}

/// "Datensatz übernehmen" path of the §19.11 onboarding flow.
///
/// Configures the orchestrator with the chosen adapter, pulls every
/// remote log into the applier, registers this device in
/// `meta.json`, and persists the user_prefs entries so the next app
/// start restores the same adapter and device name.
///
/// On success the scheduler is kicked so the next round emits the
/// post-onboarding heartbeat.
#[tauri::command]
pub async fn accept_remote_dataset(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
    device_name: Option<String>,
) -> CommandResult<OnboardingReport> {
    let adapter = build_adapter(&config)?;
    adapter.test_connection().await.map_err(sync_err)?;

    // Run the onboarding side first. If it fails (e.g. remote has
    // no meta.json), we haven't yet altered the orchestrator's
    // state — the next attempt can pick a different path.
    let trimmed = device_name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let report = onboarding
        .accept_remote(adapter.as_ref(), trimmed)
        .await
        .map_err(sync_err)?;

    // Commit the choice into the orchestrator + user_prefs only
    // now that onboarding has succeeded.
    orchestrator.configure(Arc::clone(&adapter));
    let shared = db.shared();
    persist_adapter_config(&UserPrefsRepo::new(&shared), &config)?;
    scheduler.kick();
    Ok(report)
}

/// "Neu beginnen" path of the §19.11 onboarding flow.
///
/// Overwrites the remote `meta.json` with a fresh one naming only
/// this device, then wires the orchestrator + scheduler so the
/// already-queued pending logs push on the next round.
///
/// Caller MUST gate this behind an explicit confirmation prompt —
/// existing remote logs become orphans the moment the new
/// `meta.json` lands (they're physically still there but no device
/// claims them in the registry; compaction in Phase Sg eventually
/// reaps them).
#[tauri::command]
pub async fn adopt_local_dataset(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    onboarding: State<'_, Arc<OnboardingService>>,
    config: SyncAdapterConfig,
    device_name: Option<String>,
) -> CommandResult<OnboardingReport> {
    let adapter = build_adapter(&config)?;
    adapter.test_connection().await.map_err(sync_err)?;

    let trimmed = device_name.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let report = onboarding
        .adopt_local(adapter.as_ref(), trimmed)
        .await
        .map_err(sync_err)?;

    orchestrator.configure(Arc::clone(&adapter));
    let shared = db.shared();
    persist_adapter_config(&UserPrefsRepo::new(&shared), &config)?;
    scheduler.kick();
    Ok(report)
}

/// Helper used by `lib.rs::setup` to reconstruct the adapter
/// from the persisted prefs on app start. Returns `Ok(None)`
/// when no adapter was configured before — the orchestrator
/// stays in its initial unconfigured state.
pub fn build_adapter_from_prefs(
    db: &crate::db::SharedConn,
) -> Option<Arc<dyn SyncAdapter>> {
    let prefs = UserPrefsRepo::new(db);
    let kind = prefs.get(PREF_ADAPTER_KIND).ok().flatten()?;
    match kind.as_str() {
        "local" => {
            let path = prefs.get(PREF_LOCAL_PATH).ok().flatten()?;
            if path.trim().is_empty() {
                return None;
            }
            Some(Arc::new(LocalFsSyncAdapter::new(PathBuf::from(path))))
        }
        "webdav" => {
            let url = prefs.get(PREF_WEBDAV_URL).ok().flatten()?;
            if url.trim().is_empty() {
                return None;
            }
            let user = prefs
                .get(PREF_WEBDAV_USER)
                .ok()
                .flatten()
                .unwrap_or_default();
            let password = secrets::retrieve(
                WEBDAV_SECRET_ACCOUNT,
                SecretSlot::Password,
            )
            .ok();
            let credentials = match (user.trim().is_empty(), password) {
                (false, Some(pw)) => WebDavCredentials::basic(user.trim(), &pw),
                _ => WebDavCredentials::None,
            };
            WebDavSyncAdapter::new(url.trim(), credentials)
                .ok()
                .map(|a| Arc::new(a) as Arc<dyn SyncAdapter>)
        }
        // Forward-compat: an unknown kind (left over from a
        // future Aperio version) is silently treated as "no
        // adapter configured" rather than a hard error. The
        // user reconfigures in Settings; we don't crash the
        // app over it.
        _ => None,
    }
}
