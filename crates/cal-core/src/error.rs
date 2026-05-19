//! Shared error types.

use thiserror::Error;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Top-level error type for adapter operations.
///
/// Adapters translate their internal errors (HTTP, XML, OAuth, …) into one
/// of these variants so the caller can handle them uniformly.
#[derive(Debug, Error)]
pub enum Error {
    #[error("authentication failed: {0}")]
    Authentication(String),

    #[error("access denied: {0}")]
    Forbidden(String),

    #[error("resource not found: {0}")]
    NotFound(String),

    #[error("conflict (ETag / precondition failed): {0}")]
    Conflict(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("operation not supported by adapter: {0}")]
    Unsupported(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl Error {
    pub fn authentication(msg: impl Into<String>) -> Self {
        Self::Authentication(msg.into())
    }

    pub fn network(msg: impl Into<String>) -> Self {
        Self::Network(msg.into())
    }

    pub fn protocol(msg: impl Into<String>) -> Self {
        Self::Protocol(msg.into())
    }

    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(msg.into())
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }
}
