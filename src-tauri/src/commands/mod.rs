//! Tauri commands invoked from the frontend.
//!
//! Each command translates a `cal_core::Error` into a [`CommandError`] —
//! a structured, serialisable error that the frontend can pattern-match
//! against. We intentionally do **not** stringify errors with `.to_string()`
//! here, because the frontend needs to distinguish "not found" from
//! "conflict" from "auth failure" to render appropriate UI.

mod accounts;
mod birthdays;
mod cache;
pub(crate) mod cache_swr;
mod calendars;
mod color_labels;
mod conflicts;
mod contacts;
mod context_menu;
mod external;
mod logs;
mod overrides;
mod plugins;
mod reminders;
mod search;
mod sounds;
mod sync;
mod tasks;
mod user_prefs;
mod videoconference;

use plugin_core::manager::{DiscoverError, InteractiveAuthError, ProbeHostKeyError};
use plugin_core::PluginManager;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

pub use accounts::*;
pub use cache::*;
pub use calendars::*;
pub use color_labels::*;
pub use conflicts::*;
pub use contacts::*;
pub use context_menu::*;
pub use external::*;
pub use logs::*;
pub use overrides::*;
pub use plugins::*;
pub use reminders::*;
pub use search::*;
pub use sounds::*;
pub use sync::*;
pub use tasks::*;
pub use user_prefs::*;
pub use videoconference::*;

/// Frontend-friendly error envelope.
///
/// `code` is a stable, machine-readable identifier; `message` is the
/// human-readable description (already localised in English here — the
/// frontend translates known `code`s and falls back to `message` for
/// unknowns).
#[derive(Debug, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<cal_core::Error> for CommandError {
    fn from(err: cal_core::Error) -> Self {
        use cal_core::Error::*;
        let (code, message) = match err {
            Authentication(m) => ("auth", m),
            Forbidden(m) => ("forbidden", m),
            NotFound(m) => ("not_found", m),
            Conflict(m) => ("conflict", m),
            Network(m) => ("network", m),
            Protocol(m) => ("protocol", m),
            InvalidInput(m) => ("invalid_input", m),
            Unsupported(m) => ("unsupported", m),
            Internal(m) => ("internal", m),
        };
        Self { code, message }
    }
}

impl From<crate::DbError> for CommandError {
    fn from(err: crate::DbError) -> Self {
        Self {
            code: "internal",
            message: err.to_string(),
        }
    }
}

impl From<vc_core::VcError> for CommandError {
    fn from(err: vc_core::VcError) -> Self {
        use vc_core::VcError::*;
        let (code, message) = match err {
            Authentication(m) => ("auth", m),
            Forbidden(m) => ("forbidden", m),
            NotFound(m) => ("not_found", m),
            Network(m) => ("network", m),
            Protocol(m) => ("protocol", m),
            InvalidInput(m) => ("invalid_input", m),
            Unsupported(m) => ("unsupported", m),
            Internal(m) => ("internal", m),
        };
        Self { code, message }
    }
}

/// Shorthand used by every command implementation.
pub type CommandResult<T> = std::result::Result<T, CommandError>;

/// Run an OAuth-style interactive auth dance via the plugin
/// manager and parse the resulting credential blob into a
/// `serde_json::Value`. Each plugin returns its provider-
/// specific TokenSet shape (Google has `access_token` +
/// `refresh_token` + `expires_at` + `scope`, Dropbox just
/// `refresh_token` + `access_token` + `expires_at`, …); callers
/// extract the fields they need via `.get(...)`.
pub async fn run_plugin_auth(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    args_json: Value,
) -> Result<Value, CommandError> {
    let bytes = plugin_manager
        .interactive_auth(plugin_id, &args_json.to_string())
        .await
        .map_err(interactive_auth_error_to_command)?;
    serde_json::from_slice(&bytes).map_err(|e| CommandError {
        code: "protocol",
        message: format!("plugin {plugin_id} returned non-JSON token blob: {e}"),
    })
}

pub fn interactive_auth_error_to_command(err: InteractiveAuthError) -> CommandError {
    match err {
        InteractiveAuthError::PluginMissing(id) => CommandError {
            code: "plugin_missing",
            message: format!("plugin {id} is not loaded"),
        },
        InteractiveAuthError::Unsupported(id) => CommandError {
            code: "unsupported",
            message: format!("plugin {id} doesn't support interactive auth"),
        },
        // The plugin's own error message (Google's "invalid_grant",
        // Microsoft's CSRF mismatch, browser-closed timeout, …)
        // surfaces verbatim under the generic `auth` code so the
        // frontend renders it next to the Sign-in button.
        InteractiveAuthError::Plugin(msg) => CommandError {
            code: "auth",
            message: msg,
        },
    }
}

/// Run a service-discovery cascade via the plugin manager and
/// deserialise the result into the caller's `T` (the host stays
/// adapter-crate-agnostic — only the plugin knows the discovery
/// protocol's response shape; the host names the JSON layout it
/// expects via `T`).
pub async fn run_plugin_discover<T: DeserializeOwned>(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    args_json: Value,
) -> Result<T, CommandError> {
    let bytes = plugin_manager
        .discover(plugin_id, &args_json.to_string())
        .await
        .map_err(discover_error_to_command)?;
    serde_json::from_slice(&bytes).map_err(|e| CommandError {
        code: "protocol",
        message: format!("plugin {plugin_id} returned non-JSON discover blob: {e}"),
    })
}

pub fn discover_error_to_command(err: DiscoverError) -> CommandError {
    match err {
        DiscoverError::PluginMissing(id) => CommandError {
            code: "plugin_missing",
            message: format!("plugin {id} is not loaded"),
        },
        DiscoverError::Unsupported(id) => CommandError {
            code: "unsupported",
            message: format!("plugin {id} doesn't support discover"),
        },
        // Discovery failures land under `not_found` so the
        // AccountsDialog can suggest "enter the endpoint
        // manually" — the plugin's own message ("Autodiscover
        // HTTP 401", "no endpoint for hs-anhalt.de", …) carries
        // the actionable text.
        DiscoverError::Plugin(msg) => CommandError {
            code: "not_found",
            message: msg,
        },
    }
}

/// Run a TOFU host-key probe via the plugin manager and
/// deserialise the result into the caller's `T` (typically
/// `{"fingerprint": "SHA256:..."}`). Same shape as
/// [`run_plugin_discover`].
pub async fn run_plugin_probe_host_key<T: DeserializeOwned>(
    plugin_manager: &PluginManager,
    plugin_id: &str,
    args_json: Value,
) -> Result<T, CommandError> {
    let bytes = plugin_manager
        .probe_host_key(plugin_id, &args_json.to_string())
        .await
        .map_err(probe_host_key_error_to_command)?;
    serde_json::from_slice(&bytes).map_err(|e| CommandError {
        code: "protocol",
        message: format!("plugin {plugin_id} returned non-JSON probe blob: {e}"),
    })
}

pub fn probe_host_key_error_to_command(err: ProbeHostKeyError) -> CommandError {
    match err {
        ProbeHostKeyError::PluginMissing(id) => CommandError {
            code: "plugin_missing",
            message: format!("plugin {id} is not loaded"),
        },
        ProbeHostKeyError::Unsupported(id) => CommandError {
            code: "unsupported",
            message: format!("plugin {id} doesn't support probe_host_key"),
        },
        // Probe failures land under `network` — most are
        // connection problems (dead host, TLS handshake, …). The
        // plugin's own message text carries the actionable bits.
        ProbeHostKeyError::Plugin(msg) => CommandError {
            code: "network",
            message: msg,
        },
    }
}
