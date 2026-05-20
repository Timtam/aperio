//! CalDAV-specific errors.
//!
//! The adapter speaks CalDAV's two failure modes (transport / HTTP and
//! XML parsing) and one CalDAV-shaped semantic mode (the response
//! parsed fine but the resource we were after isn't there — no
//! principal URL, no calendar-home-set, …). We surface them as
//! distinct variants so callers can branch on the right hint to show
//! a user: "check the server URL" vs "check the credentials" vs
//! "this server is reachable but doesn't speak CalDAV".

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaldavError {
    #[error("network error: {0}")]
    Network(String),

    /// The server returned an HTTP status we did not expect. Carries
    /// the status code so the UI can distinguish 401 (re-auth) from
    /// 404 (typo in the URL) from 5xx (server problem).
    #[error("unexpected HTTP status {status}: {message}")]
    Http {
        status: u16,
        message: String,
    },

    /// We got a response but its body wasn't the CalDAV XML we
    /// needed — usually means the URL points at a generic web server
    /// rather than a CalDAV endpoint.
    #[error("malformed response: {0}")]
    Protocol(String),

    /// The CalDAV chain ran out of breadcrumbs before we could find
    /// what we needed (e.g. the principal URL was missing or the
    /// calendar-home-set returned no entries).
    #[error("discovery failed: {0}")]
    Discovery(String),

    #[error("invalid configuration: {0}")]
    Config(String),
}

impl From<reqwest::Error> for CaldavError {
    fn from(err: reqwest::Error) -> Self {
        CaldavError::Network(err.to_string())
    }
}

impl From<url::ParseError> for CaldavError {
    fn from(err: url::ParseError) -> Self {
        CaldavError::Config(err.to_string())
    }
}

pub type CaldavResult<T> = std::result::Result<T, CaldavError>;
