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

    /// The operating system has not granted the app this data: the device's
    /// own calendars or reminders without full access. Not a login problem —
    /// no password helps — and not a store that is still loading: the user
    /// grants it in the system settings (decisions 166 and 171).
    #[error("access not granted by the operating system: {0}")]
    AccessNotGranted(String),
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

    pub fn access_not_granted(msg: impl Into<String>) -> Self {
        Self::AccessNotGranted(msg.into())
    }

    pub fn unsupported(msg: impl Into<String>) -> Self {
        Self::Unsupported(msg.into())
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    /// The provider turned the request away because it is throttling, and
    /// the request wrote nothing (decision 270). Carried as a refusal, with
    /// the token [`crate::WriteRefusal::ProviderBusy`]: a plugin's error
    /// crosses to the host as its status and its text only, and `forbidden`
    /// is the status every surface reads as "never landed". So host code asks
    /// [`Self::is_provider_busy`] and never matches a variant. Only for a
    /// request that wrote nothing: a throttle after a request of the same
    /// save had landed is [`crate::WriteLeftover::PartlySaved`].
    pub fn provider_busy(detail: crate::BusyDetail) -> Self {
        Self::Forbidden(crate::WriteRefusal::ProviderBusy.message(detail.word()))
    }

    /// Whether this error says the provider is busy (decision 270), whichever
    /// error carries the token.
    pub fn is_provider_busy(&self) -> bool {
        matches!(
            crate::WriteRefusal::parse(self.message()),
            Some((crate::WriteRefusal::ProviderBusy, _))
        )
    }

    /// The text the error carries, without its kind.
    pub fn message(&self) -> &str {
        match self {
            Self::Authentication(m)
            | Self::Forbidden(m)
            | Self::NotFound(m)
            | Self::Conflict(m)
            | Self::Network(m)
            | Self::Protocol(m)
            | Self::InvalidInput(m)
            | Self::Unsupported(m)
            | Self::Internal(m)
            | Self::AccessNotGranted(m) => m,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BusyDetail, WriteRefusal};

    #[test]
    fn a_busy_provider_is_a_refusal_with_its_token() {
        let err = Error::provider_busy(BusyDetail::TooManyRequests);
        assert!(matches!(&err, Error::Forbidden(m) if m == "provider-busy: http-429"));
        assert!(err.is_provider_busy());
        assert_eq!(
            WriteRefusal::parse(err.message()),
            Some((WriteRefusal::ProviderBusy, "http-429"))
        );
    }

    #[test]
    fn the_busy_token_is_read_whatever_carries_it() {
        for err in [
            Error::Forbidden("provider-busy: rate-limit".into()),
            Error::Network("provider-busy: server-busy".into()),
            Error::Protocol("provider-busy".into()),
        ] {
            assert!(err.is_provider_busy(), "{err:?}");
        }
        for err in [
            Error::Authentication("rateLimitExceeded".into()),
            Error::Forbidden("server-refused: ErrorServerBusy".into()),
            Error::Protocol("Google HTTP 429: too many".into()),
            Error::Forbidden("provider-busy-elsewhere: x".into()),
        ] {
            assert!(!err.is_provider_busy(), "{err:?}");
        }
    }
}
