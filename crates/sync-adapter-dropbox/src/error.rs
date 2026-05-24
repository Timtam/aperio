//! Dropbox-specific error type. Mapped into `sync_core::SyncError`
//! at the trait boundary so the rest of the sync layer uses one
//! unified vocabulary.

use thiserror::Error;

pub type DropboxResult<T> = Result<T, DropboxError>;

/// Failures that can come out of a Dropbox API round-trip.
///
/// `Auth`, `NotFound` and `AuthDenied` are the user-actionable
/// cases; the rest are best-effort context for diagnostics.
#[derive(Debug, Error)]
pub enum DropboxError {
    /// Bad config — empty client_id, malformed URL. Caller
    /// shouldn't have reached the API layer with these.
    #[error("dropbox config error: {0}")]
    Config(String),

    /// 401 Unauthorized or Dropbox's `auth/expired_access_token`
    /// JSON error. Triggers a one-shot refresh-and-retry in the
    /// trait method bodies.
    #[error("dropbox auth failed: {0}")]
    Auth(String),

    /// `path/not_found` response. Folded into `Ok(None)` by the
    /// upper layer on read paths so the caller can branch on
    /// "absence" instead of catching errors.
    #[error("dropbox not found: {0}")]
    NotFound(String),

    /// Generic HTTP error that doesn't fit the other variants —
    /// 5xx, network blip, etc.
    #[error("dropbox HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// The wire format coming back didn't match what the API
    /// promises (malformed JSON, missing required field).
    #[error("dropbox protocol error: {0}")]
    Protocol(String),

    /// Underlying IO error from the loopback listener during
    /// the OAuth flow.
    #[error("dropbox IO error: {0}")]
    Io(String),

    /// CSRF state mismatch on the OAuth redirect. The
    /// authorisation page came back with a `state` parameter
    /// that doesn't match the one we sent — either a stale
    /// browser tab or a tampered redirect.
    #[error("OAuth state mismatch (CSRF)")]
    Csrf,

    /// User didn't complete the consent dance within the
    /// timeout window (5 minutes).
    #[error("OAuth dance timed out")]
    AuthTimeout,

    /// Authorisation server explicitly returned an `error=`
    /// parameter (user clicked "Decline" or the app config is
    /// invalid).
    #[error("OAuth denied: {0}")]
    AuthDenied(String),
}

impl DropboxError {
    /// `true` for the authentication-class variants. Used by
    /// the trait methods to gate the one-shot refresh-and-retry.
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            Self::Auth(_) | Self::Csrf | Self::AuthTimeout | Self::AuthDenied(_)
        )
    }

    /// `true` for the `NotFound` variant only. Read paths fold
    /// this into `Ok(None)` upstream.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }
}

impl From<std::io::Error> for DropboxError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<reqwest::Error> for DropboxError {
    fn from(err: reqwest::Error) -> Self {
        // reqwest::Error can carry a status; surface it when
        // present so the caller's auth-vs-network branching
        // works without re-parsing.
        if let Some(status) = err.status() {
            Self::Http {
                status: status.as_u16(),
                message: err.to_string(),
            }
        } else {
            Self::Http {
                status: 0,
                message: err.to_string(),
            }
        }
    }
}
