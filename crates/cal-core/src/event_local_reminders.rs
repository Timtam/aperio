//! Reminders Aperio keeps for one event and tells no provider about.
//!
//! A reminder normally rides ON the event, so every client of the calendar
//! rings. These do not: they live in Aperio, travel between the user's own
//! devices through Aperio's sync, and reach nobody else on a shared calendar.
//! See migration `0043_event_local_reminders.sql`.
//!
//! The type lives here rather than in the host because it crosses crates, the
//! same arrangement [`crate::EventGroup`] has: the host writes it, the sync
//! applier reads it off the wire, and the local adapter stores what arrives.

use serde::{Deserialize, Serialize};

use crate::Reminder;

/// One event's Aperio-only reminders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventLocalReminders {
    pub calendar_id: String,
    /// Series master id — a recurring appointment is reminded of as a series,
    /// exactly as its own reminders are.
    pub event_id: String,
    /// An empty list is a decision ("none of my own here"), not an absence:
    /// the row stays so a peer that has not heard yet has something to lose
    /// against. See the migration.
    pub reminders: Vec<Reminder>,
    /// The title the event had when this was set. Half of the SIGNATURE, and
    /// never shown: what the user reads comes from the event itself, which may
    /// since have been renamed.
    pub title: String,
    /// The start it had then. The other half.
    pub starts_at: String,
    pub updated_at: String,
}

impl EventLocalReminders {
    /// Whether this row asks for anything at all.
    pub fn is_empty(&self) -> bool {
        self.reminders.is_empty()
    }
}
