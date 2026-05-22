//! Todoist-adapter errors.
//!
//! Same shape the Vikunja adapter uses — a crate-local enum mapped
//! at the trait boundary. Todoist's REST surface has no special
//! transport quirks (no SOAP fault, no OAuth-CSRF), so the variant
//! list stays tight: network, HTTP, protocol, config.
//!
//! Todoist's error envelope is `{ "error_tag": "...", "error":
//! "...", "http_code": <int>, "error_code": <int> }`. We surface
//! `error_tag` / `error` verbatim in `Http.message` — the command
//! mapper inspects the status (401 / 404 / …) and routes the error,
//! it doesn't try to parse the body.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum TodoistError {
    #[error("network error: {0}")]
    Network(String),

    #[error("Todoist returned HTTP {status}: {message}")]
    Http { status: u16, message: String },

    #[error("malformed response: {0}")]
    Protocol(String),

    #[error("invalid configuration: {0}")]
    Config(String),
}

pub type TodoistResult<T> = std::result::Result<T, TodoistError>;

impl From<reqwest::Error> for TodoistError {
    fn from(err: reqwest::Error) -> Self {
        TodoistError::Network(err.to_string())
    }
}

impl From<url::ParseError> for TodoistError {
    fn from(err: url::ParseError) -> Self {
        TodoistError::Config(err.to_string())
    }
}

impl From<serde_json::Error> for TodoistError {
    fn from(err: serde_json::Error) -> Self {
        TodoistError::Protocol(format!("json: {err}"))
    }
}
