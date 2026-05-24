//! Cross-device sync commands (DESIGN.md §19, Phase Sd).
//!
//! Three verbs exposed to the frontend:
//!
//!   - `configure_sync_adapter(kind, config)` — install / swap
//!     the runtime adapter and persist the choice in
//!     `user_prefs`. `kind` is the adapter family
//!     (`"local"` for Phase Sd; `"webdav"`, `"sftp"`, …
//!     follow in later phases). `config` is family-specific.
//!   - `sync_now()` — manual trigger. Returns a
//!     `SyncRoundReport` so the dialogue can show "12 events
//!     applied" without a follow-up status fetch.
//!   - `get_sync_status()` — read-only snapshot for the
//!     status indicator.
//!
//! Adapter configuration values DO NOT propagate via the event
//! log (per §19.2.1) — they're device-local. The user_prefs
//! whitelist already excludes everything under `sync.adapter.*`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Deserialize;
use sync_adapter_local::LocalFsSyncAdapter;
use sync_core::SyncAdapter;
use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::event_log::{SyncOrchestrator, SyncRoundReport, SyncScheduler, SyncStatus};
use crate::user_prefs::UserPrefsRepo;

/// `user_prefs` key naming the currently-configured adapter
/// family. Empty / missing → no adapter, sync disabled.
const PREF_ADAPTER_KIND: &str = "sync.adapter.kind";

/// Family-specific config (per-kind sub-key). For `kind="local"`
/// we just need a filesystem path.
const PREF_LOCAL_PATH: &str = "sync.adapter.local.path";

/// Request body for `configure_sync_adapter`. The kind is
/// flattened so the frontend can build one of:
///
/// ```jsonc
/// { "kind": "local",  "path":   "/mnt/nas/aperio" }
/// { "kind": "none" }   // disconnects any configured adapter
/// ```
///
/// Future adapter kinds (`webdav`, `sftp`, …) will add their own
/// branches as new struct variants.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncAdapterConfig {
    /// Filesystem path-based adapter — DESIGN.md §19.6 entry.
    Local { path: String },
    /// Explicit disconnect. The orchestrator drops its adapter
    /// handle; subsequent `sync_now` calls return a clear "not
    /// configured" error rather than silently no-oping.
    None,
}

/// Install / swap the active sync adapter. Persists the user's
/// choice so the next app start reconstructs the same adapter
/// in `lib.rs`'s setup phase.
#[tauri::command]
pub async fn configure_sync_adapter(
    db: State<'_, DbHandle>,
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
    scheduler: State<'_, Arc<SyncScheduler>>,
    config: SyncAdapterConfig,
) -> CommandResult<()> {
    let shared = db.shared();
    let prefs = UserPrefsRepo::new(&shared);
    match config {
        SyncAdapterConfig::Local { path } => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(CommandError {
                    code: "invalid_input",
                    message: "sync path must not be empty".into(),
                });
            }
            let adapter = LocalFsSyncAdapter::new(PathBuf::from(trimmed));
            // Probe the path before persisting — we want
            // misconfigurations to surface immediately at the
            // settings dialog, not hours later when the first
            // sync_now runs.
            adapter.test_connection().await.map_err(|err| CommandError {
                code: "io",
                message: err.to_string(),
            })?;
            orchestrator.configure(Arc::new(adapter));
            prefs.set(PREF_ADAPTER_KIND, "local").map_err(|err| {
                CommandError {
                    code: "internal",
                    message: err.to_string(),
                }
            })?;
            prefs.set(PREF_LOCAL_PATH, trimmed).map_err(|err| {
                CommandError {
                    code: "internal",
                    message: err.to_string(),
                }
            })?;
            // Kick the scheduler so the user sees data flow
            // immediately instead of waiting up to one interval
            // for the periodic loop. The debounce window swallows
            // any pile of mutations the writer queued while the
            // adapter was unconfigured.
            scheduler.kick();
        }
        SyncAdapterConfig::None => {
            orchestrator.deconfigure();
            prefs.delete(PREF_ADAPTER_KIND).map_err(|err| CommandError {
                code: "internal",
                message: err.to_string(),
            })?;
            // Keep PREF_LOCAL_PATH around so re-enabling the
            // same path is one click away. It's already
            // per-device + never synced, so leaving it has no
            // downside.
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
    orchestrator
        .sync_now()
        .await
        .map_err(|err| CommandError {
            code: "internal",
            message: err.to_string(),
        })
}

/// Read-only status snapshot for the status indicator.
#[tauri::command]
pub async fn get_sync_status(
    orchestrator: State<'_, Arc<SyncOrchestrator>>,
) -> CommandResult<SyncStatus> {
    Ok(orchestrator.status())
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
        // Forward-compat: an unknown kind (left over from a
        // future Aperio version) is silently treated as "no
        // adapter configured" rather than a hard error. The
        // user reconfigures in Settings; we don't crash the
        // app over it.
        _ => None,
    }
}
