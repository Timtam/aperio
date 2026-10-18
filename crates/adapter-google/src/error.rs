//! Errors that bubble out of the Google adapter.
//!
//! Same pattern as the other adapters: a crate-local error enum with
//! a `From<reqwest::Error>` impl, mapped to `cal_core::Error` at the
//! trait boundary so the rest of Aperio doesn't have to learn
//! Google-specific failure modes.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GoogleError {
    #[error("network error: {0}")]
    Network(String),

    /// A request that never left: no connection could be made, or the
    /// request could not be built. Nothing reached Google, unlike
    /// [`GoogleError::Network`], where an answer was lost and the request
    /// may have been carried out.
    #[error("network error: {0}")]
    Unreached(String),

    /// Unexpected HTTP status from a Google API call. Carries the
    /// status code so the trait-impl mapping can route 401 to
    /// re-authentication and 404 to "calendar gone".
    #[error("Google API returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// JSON we didn't expect — usually means Google changed the
    /// response shape or returned an error envelope we don't recognise.
    #[error("malformed response: {0}")]
    Protocol(String),

    /// OAuth flow failed at the consent screen (user clicked deny,
    /// closed the tab, etc.). The string is the `error` query
    /// parameter Google sent back, verbatim.
    #[error("authorisation denied: {0}")]
    AuthDenied(String),

    /// The auth flow timed out — typically 5 min — without a redirect
    /// hitting the localhost listener. User probably abandoned the
    /// browser tab.
    #[error("authorisation timed out")]
    AuthTimeout,

    /// CSRF protection check failed. The `state` parameter Google
    /// echoed back didn't match what we'd sent in the auth request.
    /// Either a CSRF attack or a duplicate / cached redirect.
    #[error("CSRF state mismatch in redirect")]
    Csrf,

    /// Local OS error while binding the redirect listener or opening
    /// the browser.
    #[error("local OS error: {0}")]
    Io(String),

    /// Anything related to the local config that's structurally
    /// invalid — empty client_id, malformed URL, etc.
    #[error("invalid configuration: {0}")]
    Config(String),

    /// A new series was created, its deleted occurrences could not all be
    /// cancelled (decisions 202, 203), and it was deleted again (`undone`) or
    /// could not be (`!undone`, it may stand). `detail` is
    /// [`cal_core::step_detail`]. Mapped to a refusal or a leftover, never to
    /// an error a caller reads as "never landed" when something may stand.
    #[error("the new series kept not its deleted occurrences ({detail})")]
    NotKept { undone: bool, detail: String },
}

pub type GoogleResult<T> = std::result::Result<T, GoogleError>;

impl From<reqwest::Error> for GoogleError {
    fn from(err: reqwest::Error) -> Self {
        if err.is_connect() || err.is_builder() {
            GoogleError::Unreached(err.to_string())
        } else {
            GoogleError::Network(err.to_string())
        }
    }
}

impl From<std::io::Error> for GoogleError {
    fn from(err: std::io::Error) -> Self {
        GoogleError::Io(err.to_string())
    }
}

impl From<serde_json::Error> for GoogleError {
    fn from(err: serde_json::Error) -> Self {
        GoogleError::Protocol(format!("json: {err}"))
    }
}
