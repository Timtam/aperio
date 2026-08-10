//! "These two are NOT the same appointment."
//!
//! Aperio recognises a copy — same name, same start, another calendar — and
//! offers it. This is the record of the answer NO, so the offer is made once
//! rather than every morning. See `DESIGN-event-groups.md` and migration 0037.

use serde::{Deserialize, Serialize};

/// A pair the user has said is not one appointment.
///
/// Stored and compared in a CANONICAL order — the smaller `(calendar, event)`
/// first as text — so "A and B" and "B and A" are one decision. Without that
/// the same pair could be declined from one side and offered again from the
/// other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestionDecline {
    pub calendar_a: String,
    pub event_a: String,
    pub calendar_b: String,
    pub event_b: String,
    pub declined_at: String,
}

impl SuggestionDecline {
    /// Build one in canonical order, whichever way round the caller names it.
    pub fn new(first: (&str, &str), second: (&str, &str), declined_at: impl Into<String>) -> Self {
        let (a, b) = if (first.0, first.1) <= (second.0, second.1) {
            (first, second)
        } else {
            (second, first)
        };
        Self {
            calendar_a: a.0.to_string(),
            event_a: a.1.to_string(),
            calendar_b: b.0.to_string(),
            event_b: b.1.to_string(),
            declined_at: declined_at.into(),
        }
    }
}
