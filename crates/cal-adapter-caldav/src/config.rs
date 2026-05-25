//! Per-account configuration the adapter needs to do its work.
//!
//! The server URL + username live in the SQLite `accounts.config_json`
//! column so the user sees their entries in a clear, non-secret place.
//! Passwords and bearer tokens never go in here — they come from the
//! platform keychain via the `secrets` module in the host crate.

use serde::{Deserialize, Serialize};

/// JSON-friendly subset that maps directly to the
/// `accounts.config_json` column. The secret half of the credentials
/// is fetched from the keychain at the call site and combined with
/// this into [`Credentials`] below.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaldavAccountConfig {
    /// Base URL the user typed. Can be a hostname only (we run
    /// well-known discovery on it), the CalDAV root (we use it
    /// verbatim), or a specific collection URL — discovery handles
    /// all three.
    pub server_url: String,
    pub username: String,
    /// How the server expects authentication. iCloud and most generic
    /// CalDAV servers want Basic + app-specific password; a few
    /// modern stacks accept Bearer (used by future OAuth flows).
    #[serde(default)]
    pub auth_kind: AuthKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AuthKind {
    #[default]
    Basic,
    Bearer,
}

/// Combined config + secret. Constructed at call time; never stored
/// as-is.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub config: CaldavAccountConfig,
    pub secret: String,
}

impl Credentials {
    pub fn new(config: CaldavAccountConfig, secret: String) -> Self {
        Self { config, secret }
    }
}
