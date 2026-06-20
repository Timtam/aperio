//! Tauri commands backing Settings → Protokolle (the diagnostics log UI).
//!
//! Verbosity is a device-local user pref (`logging.level`) — NOT on the sync
//! whitelist, since one device's debug session shouldn't crank up logging on
//! another. The live filter is changed through the managed [`LogState`]; the
//! pref is what survives a restart (re-applied in `run()`).

use tauri::State;

use super::{CommandError, CommandResult};
use crate::db::DbHandle;
use crate::logging::{self, LogState, DEFAULT_LEVEL};
use crate::user_prefs::UserPrefsRepo;

/// Device-local pref holding the chosen log level (`error`…`trace`). Defined in
/// host-core + re-exported here so the desktop command + the mobile facade
/// share the exact same key string.
pub use host_core::logging::PREF_LOG_LEVEL;

fn internal<E: std::fmt::Display>(err: E) -> CommandError {
    CommandError {
        code: "internal",
        message: err.to_string(),
    }
}

/// The persisted log level, or the default when unset.
#[tauri::command]
pub fn get_log_level(db: State<'_, DbHandle>) -> CommandResult<String> {
    let shared = db.shared();
    let level = UserPrefsRepo::new(&shared)
        .get(PREF_LOG_LEVEL)
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_LEVEL.to_string());
    Ok(level)
}

/// Change the live verbosity and persist the choice. Validated against the
/// known level set so a bad value can't be stored or silence logging.
#[tauri::command]
pub fn set_log_level(
    db: State<'_, DbHandle>,
    log_state: State<'_, LogState>,
    level: String,
) -> CommandResult<()> {
    if !matches!(
        level.as_str(),
        "error" | "warn" | "info" | "debug" | "trace"
    ) {
        return Err(CommandError {
            code: "invalid_input",
            message: format!("unknown log level '{level}'"),
        });
    }
    log_state.set_filter(&level);
    let shared = db.shared();
    UserPrefsRepo::new(&shared)
        .set(PREF_LOG_LEVEL, &level)
        .map_err(internal)?;
    Ok(())
}

/// Tail of the newest log file for the in-app viewer (default 500 lines).
#[tauri::command]
pub fn get_recent_logs(
    log_state: State<'_, LogState>,
    lines: Option<usize>,
) -> CommandResult<String> {
    Ok(logging::recent_lines(
        &log_state.logs_dir,
        lines.unwrap_or(500),
    ))
}

/// The full (optionally redacted) log bundle as a string — used for
/// copy-to-clipboard. Defaults to redacted.
/// Clipboard bundles are capped to the most-recent ~2 MB — plenty of context
/// for support, and it keeps a huge trace bundle from choking the IPC bridge
/// / clipboard. The file export ([`export_logs`]) writes the complete log.
const CLIPBOARD_MAX_BYTES: usize = 2 * 1024 * 1024;

#[tauri::command]
pub fn collect_logs(log_state: State<'_, LogState>, redact: Option<bool>) -> CommandResult<String> {
    Ok(logging::collect(
        &log_state.logs_dir,
        redact.unwrap_or(true),
        Some(CLIPBOARD_MAX_BYTES),
    ))
}

/// Write the (optionally redacted) log bundle to a user-chosen path. The path
/// comes from the OS save dialog on the frontend, so writing it is the user's
/// explicit intent.
#[tauri::command]
pub fn export_logs(
    log_state: State<'_, LogState>,
    dest_path: String,
    redact: Option<bool>,
) -> CommandResult<()> {
    let content = logging::collect(&log_state.logs_dir, redact.unwrap_or(true), None);
    std::fs::write(&dest_path, content).map_err(|e| CommandError {
        code: "io",
        message: e.to_string(),
    })?;
    Ok(())
}

/// Remove the rotated log files (the active one is kept — see `logging::clear`).
#[tauri::command]
pub fn clear_logs(log_state: State<'_, LogState>) -> CommandResult<()> {
    logging::clear(&log_state.logs_dir);
    Ok(())
}

/// The on-disk logs directory, for display + "copy path".
#[tauri::command]
pub fn logs_dir_path(log_state: State<'_, LogState>) -> CommandResult<String> {
    Ok(log_state.logs_dir.display().to_string())
}
