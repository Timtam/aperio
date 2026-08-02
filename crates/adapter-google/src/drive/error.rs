//! Google Drive–specific error type. Mapped into
//! `sync_core::SyncError` at the trait boundary so the rest of
//! the sync layer sees one unified vocabulary.

use thiserror::Error;

pub type GoogleDriveResult<T> = Result<T, GoogleDriveError>;

#[derive(Debug, Error)]
pub enum GoogleDriveError {
    #[error("Google Drive config error: {0}")]
    Config(String),

    /// 401 Unauthorized or Drive's `invalid_grant` /
    /// `invalid_token` responses. Triggers a one-shot
    /// refresh-and-retry in the trait method bodies.
    #[error("Google Drive auth failed: {0}")]
    Auth(String),

    /// Drive uses 404 for "file not found by ID" + the
    /// "no matching file in list query" case (where we infer
    /// it from an empty `files[]` array). Both fold into
    /// `Ok(None)` at the upper layer.
    #[error("Google Drive not found: {0}")]
    NotFound(String),

    #[error("Google Drive HTTP {status}: {message}")]
    Http { status: u16, message: String },

    #[error("Google Drive protocol error: {0}")]
    Protocol(String),

    #[error("Google Drive IO error: {0}")]
    Io(String),

    #[error("OAuth state mismatch (CSRF)")]
    Csrf,

    #[error("OAuth dance timed out")]
    AuthTimeout,

    #[error("OAuth denied: {0}")]
    AuthDenied(String),
}

impl GoogleDriveError {
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            Self::Auth(_) | Self::Csrf | Self::AuthTimeout | Self::AuthDenied(_)
        )
    }

    pub fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound(_))
    }
}

impl From<std::io::Error> for GoogleDriveError {
    fn from(err: std::io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

impl From<reqwest::Error> for GoogleDriveError {
    fn from(err: reqwest::Error) -> Self {
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
