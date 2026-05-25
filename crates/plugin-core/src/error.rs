//! Plugin-core error type.
//!
//! Covers everything the manifest layer can fail at + the load-time
//! ABI checks the manager will fire in P1. Adapter / vtable
//! call-time errors are deliberately NOT in this enum — those will
//! travel through their respective Feature-trait errors (e.g.
//! `cal_core::Error`, `sync_core::SyncError`) so the existing host
//! code paths don't need to learn a new vocabulary at every layer.

use thiserror::Error;

pub type PluginResult<T> = Result<T, PluginError>;

#[derive(Debug, Error)]
pub enum PluginError {
    /// The `plugin.json` file couldn't be parsed as JSON or didn't
    /// match the expected schema (missing required field, wrong
    /// type, …).
    #[error("plugin manifest is malformed: {0}")]
    Manifest(String),

    /// The manifest's `abi_version` didn't match
    /// [`crate::ABI_VERSION`]. Surfaced to the user as "Plugin XY
    /// needs a newer/older Aperio". The two fields let the UI
    /// decide which side to nudge.
    #[error(
        "plugin requires ABI version {plugin}; this Aperio speaks v{host}"
    )]
    AbiMismatch { host: u32, plugin: u32 },

    /// The manifest's `min_app_version` is newer than the running
    /// Aperio. Same diagnostic story as the sync engine's
    /// [`sync_core::Compatibility::AppTooOld`] — the user is told
    /// to update Aperio, the plugin stays untouched.
    #[error(
        "plugin needs Aperio ≥ {required}; running v{running}"
    )]
    AppTooOld { required: String, running: String },

    /// A semver value couldn't be parsed (either the host's own
    /// `CARGO_PKG_VERSION`, which would be a bug, or the manifest's
    /// `version` / `min_app_version`, which is plugin author error).
    #[error("malformed semver string {value:?}: {reason}")]
    Semver { value: String, reason: String },

    /// IO error while reading `plugin.json` from disk.
    #[error("plugin manifest IO error: {0}")]
    Io(String),

    /// The plugin's `open_instance` hook reported a non-OK status
    /// or returned a NULL handle. The host surfaces this as
    /// "Konto konnte nicht eingerichtet werden" with the plugin's
    /// own message in the detail line.
    #[error("open_instance failed (status {status}): {message}")]
    InstanceOpen { status: i32, message: String },
}

impl From<std::io::Error> for PluginError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for PluginError {
    fn from(err: serde_json::Error) -> Self {
        Self::Manifest(err.to_string())
    }
}
