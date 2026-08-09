//! Which events mean the same appointment.
//!
//! A group is Aperio's statement ABOUT foreign data: these events, in
//! different calendars and belonging to different providers, are one
//! commitment. No provider knows the concept; see `DESIGN-event-groups.md`.
//!
//! The types live here rather than in the host because they cross crates: the
//! host writes them, the sync applier reads them off the wire, and the local
//! adapter stores what arrives. One shape in one place, the same arrangement
//! [`crate::ColorLabel`] already has.

use serde::{Deserialize, Serialize};

/// One event's membership in a group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventGroupMember {
    pub calendar_id: String,
    /// Series master id — a recurring appointment is grouped as a series.
    pub event_id: String,
    /// The title it had when it joined. Half of the SIGNATURE, and never
    /// shown: what a user reads comes from the event itself, which may since
    /// have been renamed.
    pub title: String,
    /// The start it had when it joined. The other half.
    pub starts_at: String,
    pub added_at: String,
}

/// A set of events that mean one appointment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventGroup {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    /// The whole membership, always. A group is small and only meaningful
    /// entire, so it travels and is stored as one value rather than as a
    /// stream of additions and removals that could interleave.
    pub members: Vec<EventGroupMember>,
}
