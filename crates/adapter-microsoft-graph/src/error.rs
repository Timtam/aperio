//! Microsoft Graph adapter errors.
//!
//! Mirrors the Google adapter's shape: a crate-local enum + a
//! `From<reqwest::Error>` impl, mapped at the trait boundary into
//! `cal_core::Error` so consumers see uniform variants regardless
//! of which vendor's REST hiccup actually fired.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("network error: {0}")]
    Network(String),

    /// Graph returned an HTTP status we did not expect. Carries the
    /// numeric code so the trait-impl mapper can route 401 to
    /// re-auth, 404 to NotFound, etc.
    #[error("Graph API returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    /// JSON we didn't expect.
    #[error("malformed response: {0}")]
    Protocol(String),

    /// OAuth consent screen returned an `error` query parameter
    /// (user denied access, app not approved by the tenant admin,
    /// etc.).
    #[error("authorisation denied: {0}")]
    AuthDenied(String),

    /// Five-minute ceiling on the consent dance elapsed without a
    /// redirect.
    #[error("authorisation timed out")]
    AuthTimeout,

    /// CSRF `state` mismatch on the redirect.
    #[error("CSRF state mismatch in redirect")]
    Csrf,

    /// Local OS error while binding the listener or opening the
    /// browser.
    #[error("local OS error: {0}")]
    Io(String),

    #[error("invalid configuration: {0}")]
    Config(String),

    /// Graph turned the request away for throttling and did nothing
    /// (decision 270): 429, or 503 with `Retry-After` on a request that
    /// writes nothing ([`crate::api::answer_error`]). `retry_after` is what it
    /// asked to wait, where it said: only logged (decision 278).
    #[error("graph is busy ({})", detail.word())]
    Busy {
        detail: cal_core::BusyDetail,
        retry_after: Option<std::time::Duration>,
    },

    /// A save of several requests failed after an earlier one had landed
    /// (decision 277): the change stands in part. `step` names the request
    /// that failed, a word for the log; `cause` is why. Mapped through
    /// [`cal_core::Error::after_landed`], never to a refusal.
    #[error("the change was saved in part, the {step} step failed: {cause}")]
    PartlySaved {
        step: &'static str,
        cause: Box<GraphError>,
    },
}

pub type GraphResult<T> = std::result::Result<T, GraphError>;

impl GraphError {
    /// This error, met by the `step` of a save after an earlier step landed.
    pub(crate) fn after_landed(self, step: &'static str) -> Self {
        GraphError::PartlySaved {
            step,
            cause: Box::new(self),
        }
    }
}

impl From<reqwest::Error> for GraphError {
    fn from(err: reqwest::Error) -> Self {
        GraphError::Network(err.to_string())
    }
}

impl From<std::io::Error> for GraphError {
    fn from(err: std::io::Error) -> Self {
        GraphError::Io(err.to_string())
    }
}

impl From<serde_json::Error> for GraphError {
    fn from(err: serde_json::Error) -> Self {
        GraphError::Protocol(format!("json: {err}"))
    }
}
