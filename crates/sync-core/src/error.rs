//! `SyncError` — the crate's unified error type.
//!
//! Adapters translate their internal flavours (reqwest HTTP, SSH
//! handshake, IO mismatch, JSON parse) into one of these variants
//! so the command layer can pattern-match on `code` and surface a
//! coherent UI message. Mirrors the shape of `cal_core::Error` and
//! `caldav::CaldavError`.

use thiserror::Error;

/// Convenience alias used throughout the crate.
pub type SyncResult<T, E = SyncError> = std::result::Result<T, E>;

/// Unified error type for sync operations.
///
/// The variants split along "can the user do something about it?"
/// lines: `Auth` and `EncryptionRequired` ask for a re-input;
/// `Network` and `Io` are usually transient; `Protocol` and
/// `SchemaTooOld` need an app update; `Internal` is the catch-all
/// for invariants we expect to hold.
#[derive(Debug, Error)]
pub enum SyncError {
    /// Local filesystem error — couldn't read the staging directory,
    /// write a log file, etc. Wraps `std::io::Error`-style messages.
    #[error("I/O error: {0}")]
    Io(String),

    /// Remote storage backend rejected the request at the transport
    /// layer — TCP reset, TLS handshake fail, DNS lookup miss.
    #[error("network error: {0}")]
    Network(String),

    /// Storage backend authentication failed (wrong WebDAV password,
    /// expired OAuth token, missing SSH key).
    #[error("authentication failed: {0}")]
    Auth(String),

    /// The wire format coming back from the backend doesn't match
    /// what the spec promises — malformed JSON, truncated bytes,
    /// missing required field.
    #[error("protocol error: {0}")]
    Protocol(String),

    /// `meta.json` declares an `e2e_enabled: true` but no decryption
    /// key is configured locally. The user has to enter the password
    /// for the configured sync adapter before any read/write can
    /// proceed.
    #[error("e2e encryption is enabled but no key is configured")]
    EncryptionRequired,

    /// `meta.json.min_app_version` is newer than the current app's
    /// version — the sync dataset was created by a newer Aperio and
    /// can't be read until the local install is updated.
    #[error("dataset requires app version {required}; running {running}")]
    SchemaTooOld {
        required: String,
        running: String,
    },

    /// Catch-all for invariant violations / unexpected state — every
    /// use site carries a sentence of context for the log.
    #[error("internal error: {0}")]
    Internal(String),
}

impl SyncError {
    pub fn io(msg: impl Into<String>) -> Self {
        Self::Io(msg.into())
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self::Network(msg.into())
    }

    pub fn auth(msg: impl Into<String>) -> Self {
        Self::Auth(msg.into())
    }

    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}

impl From<std::io::Error> for SyncError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<serde_json::Error> for SyncError {
    // serde_json errors hit on both the wire-decode and encode paths.
    // We treat them as Protocol because they signal "bytes don't
    // match what we expected" — same diagnostic budget the cal-core
    // adapters give them.
    fn from(err: serde_json::Error) -> Self {
        Self::Protocol(format!("json: {err}"))
    }
}
