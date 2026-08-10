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
    /// When the pair was last grouped BY HAND, if ever.
    ///
    /// A refusal that cannot lose is a trap: automatic grouping consults these
    /// marks, so one arriving from a device that was offline for three weeks
    /// would tear apart a group made deliberately yesterday. Grouping two
    /// events by hand is the opposite statement, and it stamps this.
    ///
    /// The pair counts as refused iff `declined_at` is the later of the two.
    /// Neither ever moves backwards, so two devices still merge by taking the
    /// later of each and the answer does not depend on arrival order — the
    /// union rule migration 0037 relies on, kept intact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cleared_at: Option<String>,
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
            cleared_at: None,
        }
    }

    /// Whether this row currently refuses the pair.
    ///
    /// The later statement wins, and a tie counts as refused: the two are
    /// written by different code paths in the same transaction only when the
    /// pair was declined and cleared at the identical instant, which cannot
    /// happen in one device's history — a tie is two devices, and the safe
    /// reading of "both at once" is the one that asks rather than assumes.
    pub fn is_declined(&self) -> bool {
        match &self.cleared_at {
            None => true,
            Some(cleared) => self.declined_at >= *cleared,
        }
    }

    /// Merge two views of the same pair: the later of each statement.
    ///
    /// Order-independent by construction, which is what lets the set stay
    /// synchronisable without a last-writer rule (migration 0037/0038).
    pub fn merge(&mut self, other: &Self) {
        if other.declined_at > self.declined_at {
            self.declined_at = other.declined_at.clone();
        }
        match (&self.cleared_at, &other.cleared_at) {
            (_, None) => {}
            (None, Some(theirs)) => self.cleared_at = Some(theirs.clone()),
            (Some(mine), Some(theirs)) if theirs > mine => {
                self.cleared_at = Some(theirs.clone());
            }
            _ => {}
        }
    }
}
