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
    /// request that wrote nothing: an adapter wraps the error of a later
    /// request of a save with [`Self::after_landed`] before any refusal
    /// mapping, which turns a throttle there into [`Self::partly_saved`].
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

    /// A save of several requests turned away by a busy provider after an
    /// earlier request of it had landed: the change stands in part (decision
    /// 277). Carried as a network error with the token
    /// [`crate::WriteLeftover::PartlySaved`] — a leftover, which every surface
    /// reads as "may have landed", never as a refusal. `step` names the
    /// request that was turned away, a word for the log. Built through
    /// [`Self::after_landed`].
    pub fn partly_saved(step: &str) -> Self {
        Self::Network(crate::WriteLeftover::PartlySaved.message(step))
    }

    /// Whether this error says a save stands in part (decision 277),
    /// whichever error carries the token.
    pub fn is_partly_saved(&self) -> bool {
        matches!(
            crate::WriteLeftover::parse(self.message()),
            Some((crate::WriteLeftover::PartlySaved, _))
        )
    }

    /// What this error becomes when it met the `step` of a save after an
    /// earlier request of the same save had landed (decision 277): never an
    /// error a surface reads as "never landed" ([`Self::never_landed`]). A
    /// busy provider leaves the change [`Self::partly_saved`]; a network error
    /// that already says it may have landed stays as it is; anything else — a
    /// refusal above all — becomes a network error that keeps its text behind
    /// the step, so neither a refusing kind nor a token at its start survives.
    /// A network error, because it keeps its code on both shells. An adapter
    /// wraps the later request's error with it before any refusal mapping.
    pub fn after_landed(self, step: &str) -> Self {
        if self.is_provider_busy() {
            return Self::partly_saved(step);
        }
        match self {
            lost @ Self::Network(_) if !lost.never_landed() => lost,
            other => Self::Network(format!("{step}: {other}")),
        }
    }

    /// Whether this error says a write certainly changed nothing at the
    /// provider: a refusal token, or a refusing kind — and never a leftover,
    /// which says something stands. Anything else (a lost answer, an
    /// unreadable one, an internal failure) may have reached the provider. The
    /// rule `writeNeverLanded` (shared/eventWriteError.ts) reads on the
    /// shells; both read the `landed` rows of `writeOutcome.json`.
    pub fn never_landed(&self) -> bool {
        if crate::WriteLeftover::parse(self.message()).is_some() {
            return false;
        }
        if crate::WriteRefusal::parse(self.message()).is_some() {
            return true;
        }
        !matches!(
            self,
            Self::Network(_) | Self::Protocol(_) | Self::Internal(_)
        )
    }

    /// The code a host hands this error to a shell under: the desktop's
    /// `CommandError` and the phone's coded errors use these words.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Authentication(_) => "auth",
            Self::Forbidden(_) | Self::AccessNotGranted(_) => "forbidden",
            Self::NotFound(_) => "not_found",
            Self::Conflict(_) => "conflict",
            Self::Network(_) => "network",
            Self::Protocol(_) => "protocol",
            Self::InvalidInput(_) => "invalid_input",
            Self::Unsupported(_) => "unsupported",
            Self::Internal(_) => "internal",
        }
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

/// Heuristic: does a provider error text look like an AUTH failure (as
/// opposed to a network blip)? Substring match over the usual suspects —
/// conservative on purpose: a false "auth" only makes the UI suggest
/// re-checking the password. The OAuth needles matter because a revoked
/// grant's text carries the TOKEN endpoint's body
/// (`{"error":"invalid_grant",...}`): most adapters report such a refused
/// refresh as an authentication error, but some adapters and messages passed
/// through verbatim carry it as "protocol error: … HTTP 400: …" — exactly the
/// case where re-authenticating is the fix. A busy provider's text never
/// matches (`BusyDetail` words, tested).
pub fn is_auth_shaped(error: &str) -> bool {
    let lower = error.to_lowercase();
    [
        "401",
        "403",
        "unauthorized",
        "unauthorised",
        "forbidden",
        "authentication",
        "invalid credentials",
        "password",
        "invalid_grant",
        "invalid_client",
        "expired or revoked",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
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

    /// A request that failed after an earlier one of the same save landed
    /// never reads as "never landed" (decision 277): no refusing kind and no
    /// refusal token survive, and only a busy provider says "saved in part".
    #[test]
    fn a_request_after_a_landed_one_never_reads_as_never_landed() {
        for busy in [
            Error::provider_busy(BusyDetail::RateLimit),
            Error::Network("provider-busy: server-busy".into()),
        ] {
            let busy = busy.after_landed("move");
            assert!(
                matches!(&busy, Error::Network(m) if m == "partly-saved: move"),
                "{busy:?}"
            );
            assert!(busy.is_partly_saved());
        }
        // A token carried as a network error is no lost answer.
        let token = Error::Network("identity-unknown: you".into()).after_landed("move");
        assert!(!token.never_landed(), "{token:?}");
        assert!(token.message().starts_with("move: "), "{token:?}");
        // A leftover stays one.
        let stands = Error::Network("created-may-stand: time".into()).after_landed("move");
        assert!(matches!(&stands, Error::Network(m) if m == "created-may-stand: time"));
        for err in [
            Error::Authentication("token refused".into()),
            Error::Forbidden("server-refused: x".into()),
            Error::AccessNotGranted("calendar".into()),
            Error::NotFound("gone".into()),
            Error::Conflict("etag".into()),
            Error::InvalidInput("bad".into()),
            Error::Unsupported("no".into()),
            Error::Internal("oops".into()),
            Error::Protocol("unreadable".into()),
            Error::Network("timed out".into()),
        ] {
            let was = err.to_string();
            let after = err.after_landed("move");
            assert!(matches!(&after, Error::Network(_)), "{was}: {after:?}");
            assert!(!after.never_landed(), "{was}");
            assert_eq!(WriteRefusal::parse(after.message()), None, "{was}");
            assert!(!after.is_partly_saved(), "{was}");
            // The text of what went wrong is kept.
            assert!(
                after.to_string().contains(was.split(": ").last().unwrap()),
                "{was}: {after}"
            );
        }
    }

    /// The error a host hands a shell as `code` and `message`, as this core
    /// holds it.
    fn from_code(code: &str, message: String) -> Error {
        match code {
            "auth" => Error::Authentication(message),
            "forbidden" => Error::Forbidden(message),
            "not_found" => Error::NotFound(message),
            "conflict" => Error::Conflict(message),
            "network" => Error::Network(message),
            "protocol" => Error::Protocol(message),
            "invalid_input" => Error::InvalidInput(message),
            "unsupported" => Error::Unsupported(message),
            "internal" => Error::Internal(message),
            other => panic!("unknown code {other}"),
        }
    }

    /// What a later request's error becomes after an earlier one of the save
    /// landed (decision 277), as both shells read the same rows
    /// (`writeOutcome.json`; `afterLanded` in `shared/eventWriteError.ts`).
    #[test]
    fn the_shared_rows_say_what_a_landed_save_leaves() {
        let rows: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/writeOutcome.json"
        )))
        .unwrap();
        let rows = rows["after_landed"].as_array().unwrap();
        assert!(rows.len() >= 14, "the fixture lost its rows");
        for row in rows {
            let code = row["code"].as_str().unwrap();
            let message = row["message"].as_str().unwrap();
            let after = from_code(code, message.into()).after_landed(row["step"].as_str().unwrap());
            assert_eq!(
                (after.code(), after.message()),
                (
                    row["after"]["code"].as_str().unwrap(),
                    row["after"]["message"].as_str().unwrap()
                ),
                "{code}: {message}"
            );
            assert!(!after.never_landed(), "{code}: {message}");
        }
    }

    /// The rows both surfaces decide "never landed" by (`writeOutcome.json`;
    /// `src/state/eventWriteError.test.ts` reads them too), each carried
    /// under the code a host gives it.
    #[test]
    fn the_shared_rows_say_what_never_landed() {
        let rows: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/writeOutcome.json"
        )))
        .unwrap();
        let rows = rows["landed"].as_array().unwrap();
        assert!(rows.len() >= 12, "the fixture lost its rows");
        for row in rows {
            let code = row["code"].as_str().unwrap();
            let message = row["message"].as_str().unwrap().to_string();
            let err = from_code(code, message.clone());
            assert_eq!(err.code(), code);
            assert_eq!(
                err.never_landed(),
                row["never_landed"].as_bool().unwrap(),
                "{code}: {message}"
            );
        }
        // An OS refusal crosses as forbidden with its token: never landed.
        let os = Error::AccessNotGranted("calendars".into());
        assert_eq!(os.code(), "forbidden");
        assert!(os.never_landed());
    }

    #[test]
    fn a_busy_text_never_reads_as_a_login_problem() {
        for detail in BusyDetail::ALL {
            let err = Error::provider_busy(detail);
            assert!(!is_auth_shaped(&err.to_string()), "{err}");
            assert!(!is_auth_shaped(err.message()), "{err}");
        }
        assert!(is_auth_shaped("HTTP 401 Unauthorized"));
    }

    #[test]
    fn a_save_partly_made_is_a_network_error_with_its_token() {
        let err = Error::partly_saved("members");
        assert!(matches!(&err, Error::Network(m) if m == "partly-saved: members"));
        assert!(err.is_partly_saved());
        assert!(!err.is_provider_busy());
        assert!(!Error::Network("created-may-stand: x".into()).is_partly_saved());
    }
}
