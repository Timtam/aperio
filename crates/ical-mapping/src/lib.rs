//! iCalendar text to Aperio's events and back.
//!
//! The CalDAV adapter, the iCal feeds and file import, and Exchange's
//! invitations all read or write iCalendar (DESIGN-file-import.md, I6), so the
//! mapping lives here and no adapter depends on another. The text itself is
//! parsed and built by the `icalendar` crate; this crate is what Aperio reads
//! out of it and writes into it: series with their gaps, a changed occurrence
//! as a row of its own, the organizer kept out of the invitees, zones by tzdata
//! name, Windows name or the file's own definition, `SCHEDULE-AGENT`, and
//! foreign parts kept whole.

pub mod error;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod fixtures;
pub mod ical_raw;
pub mod identity;
pub mod mapping;
pub mod vtimezone;
pub mod zone_definitions;

pub use error::{IcalError, IcalResult};
