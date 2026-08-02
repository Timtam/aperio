//! Vikunja-adapter errors.
//!
//! Mirrors the shape used by the other REST adapters (Google, Graph)
//! — a crate-local enum with a `From<reqwest::Error>` shortcut, mapped
//! at the trait boundary to `cal_core::Error`. Vikunja-specific bits:
//!
//!   - The REST API returns `{ "code": <int>, "message": "..." }` on
//!     error; we don't separate that out as its own variant because
//!     callers only need the HTTP status to route auth / not-found /
//!     conflict. We surface the message verbatim in `Http.message`.
//!   - There's no "SOAP fault" / "OAuth-denied" / "CSRF" cousin —
//!     auth is a single Bearer-token header. Failures show up as
//!     `Http { status: 401, ... }` and the mapper routes that to
//!     `cal_core::Error::Authentication`.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VikunjaError {
    #[error("network error: {0}")]
    Network(String),

    #[error("Vikunja returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    #[error("malformed response: {0}")]
    Protocol(String),

    #[error("invalid configuration: {0}")]
    Config(String),
}

pub type VikunjaResult<T> = std::result::Result<T, VikunjaError>;

impl From<reqwest::Error> for VikunjaError {
    fn from(err: reqwest::Error) -> Self {
        VikunjaError::Network(err.to_string())
    }
}

impl From<url::ParseError> for VikunjaError {
    fn from(err: url::ParseError) -> Self {
        VikunjaError::Config(err.to_string())
    }
}

impl From<serde_json::Error> for VikunjaError {
    fn from(err: serde_json::Error) -> Self {
        VikunjaError::Protocol(format!("json: {err}"))
    }
}
