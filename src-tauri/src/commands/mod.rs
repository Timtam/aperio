//! Tauri commands invoked from the frontend.
//!
//! Each command translates a `cal_core::Error` into a [`CommandError`] —
//! a structured, serialisable error that the frontend can pattern-match
//! against. We intentionally do **not** stringify errors with `.to_string()`
//! here, because the frontend needs to distinguish "not found" from
//! "conflict" from "auth failure" to render appropriate UI.

mod calendars;
mod tasks;

use serde::Serialize;

pub use calendars::*;
pub use tasks::*;

/// Frontend-friendly error envelope.
///
/// `code` is a stable, machine-readable identifier; `message` is the
/// human-readable description (already localised in English here — the
/// frontend translates known `code`s and falls back to `message` for
/// unknowns).
#[derive(Debug, Serialize)]
pub struct CommandError {
    pub code: &'static str,
    pub message: String,
}

impl From<cal_core::Error> for CommandError {
    fn from(err: cal_core::Error) -> Self {
        use cal_core::Error::*;
        let (code, message) = match err {
            Authentication(m) => ("auth", m),
            Forbidden(m) => ("forbidden", m),
            NotFound(m) => ("not_found", m),
            Conflict(m) => ("conflict", m),
            Network(m) => ("network", m),
            Protocol(m) => ("protocol", m),
            InvalidInput(m) => ("invalid_input", m),
            Unsupported(m) => ("unsupported", m),
            Internal(m) => ("internal", m),
        };
        Self { code, message }
    }
}

impl From<crate::DbError> for CommandError {
    fn from(err: crate::DbError) -> Self {
        Self {
            code: "internal",
            message: err.to_string(),
        }
    }
}

/// Shorthand used by every command implementation.
pub type CommandResult<T> = std::result::Result<T, CommandError>;
