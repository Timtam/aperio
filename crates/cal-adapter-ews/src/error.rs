//! EWS-adapter errors.
//!
//! Same pattern as the other adapters: a crate-local enum mapped at
//! the trait boundary into `cal_core::Error`. EWS-specific bits
//! that surface separately:
//!
//!   - `Soap` — the server returned a SOAP fault. Carries the fault
//!     code (`ErrorAccessDenied`, `ErrorInvalidCredentials`, …) so
//!     the mapper can route auth failures separately from generic
//!     protocol errors.
//!   - `Http` — non-200 transport-level failure before the SOAP
//!     envelope even gets parsed.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum EwsError {
    #[error("network error: {0}")]
    Network(String),

    #[error("EWS returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// SOAP-level fault. EWS encodes auth, permission, not-found and
    /// many other conditions in the fault body even though the HTTP
    /// status is 200 — so we surface the structured code separately.
    #[error("EWS SOAP fault {code}: {message}")]
    Soap { code: String, message: String },

    #[error("malformed response: {0}")]
    Protocol(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    /// Autodiscover finished every probe in the cascade without
    /// finding an EWS URL. Distinct from a plain `Network` error so
    /// the UI can suggest "enter the URL manually" rather than
    /// "check your internet connection".
    #[error("Autodiscover did not find an EWS endpoint for {0}")]
    DiscoveryFailed(String),
}

pub type EwsResult<T> = std::result::Result<T, EwsError>;

impl From<reqwest::Error> for EwsError {
    fn from(err: reqwest::Error) -> Self {
        EwsError::Network(err.to_string())
    }
}

impl From<url::ParseError> for EwsError {
    fn from(err: url::ParseError) -> Self {
        EwsError::Config(err.to_string())
    }
}
