//! Day markers — a user-defined vocabulary of things worth noting about a
//! DAY, and one record per day saying which of them applied.
//!
//! This is deliberately its own small domain rather than a reuse of tasks. A
//! task has one status; this needs one per (marker, day). Modelling it as
//! recurring tasks would fill the planner with a year of instances the user
//! does not want there, push rows at providers that cannot interpret them, and
//! leave every task surface carrying an "unless it is a habit list" clause.
//! What is being described is a property of the date, so it lives on the date.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::ColorLabelId;

/// One entry in the vocabulary the user can tick a day with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayMarker {
    pub id: String,
    /// Whatever the user wants it to be — a word, a sentence, an emoji. The
    /// point of the feature is that they choose how much to say.
    pub name: String,
    /// Short stand-in for the dense views (typically one emoji). `None` ⇒ the
    /// summaries fall back to the name.
    #[serde(default)]
    pub symbol: Option<String>,
    /// Reuses the app-wide colour vocabulary rather than inventing a second
    /// one. `None` ⇒ no colour of its own.
    #[serde(default)]
    pub color_label: Option<ColorLabelId>,
    /// User-chosen order. Readers sort by this, so the list reads back the way
    /// it was built.
    #[serde(default)]
    pub position: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What a single day was marked with.
///
/// `markers` holds `DayMarker::id`s. Ids that no longer resolve are dropped by
/// the readers rather than repaired — that is also what makes a deleted marker
/// vanish from history without a migration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DayLog {
    /// The LOCAL calendar day, `YYYY-MM-DD`. The question is "how was
    /// Tuesday", not "what happened in this UTC window".
    pub day: NaiveDate,
    #[serde(default)]
    pub markers: Vec<String>,
    /// Room the design left open for a later "how was today" scale. `None`
    /// today, and every reader treats it as absent.
    #[serde(default)]
    pub rating: Option<i64>,
    pub updated_at: DateTime<Utc>,
}

impl DayLog {
    /// An untouched day. Reads exactly like a stored row with nothing ticked,
    /// so callers never branch on "was there a row".
    pub fn empty(day: NaiveDate) -> Self {
        Self {
            day,
            markers: Vec::new(),
            rating: None,
            updated_at: Utc::now(),
        }
    }

    /// Whether this day says anything at all — the test every summary uses
    /// before rendering itself.
    pub fn is_empty(&self) -> bool {
        self.markers.is_empty() && self.rating.is_none()
    }
}
