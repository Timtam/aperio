//! The host's guard in front of every event write (decisions 67a, 70a, 71a
//! and 72a).
//!
//! The rules live in `cal_core::attendee`; this module only feeds them what a
//! host has and a frontend does not: the event as the cache last read it. Both
//! hosts, the desktop commands and the mobile `Host`, call it before any
//! adapter or the local store sees an event, so neither can forget a path the
//! other covers.

use cal_core::{Event, NewEvent};

use crate::cache::CacheStore;

/// Before an update. The organizer leaves the invitees, a send intent survives
/// only for an event the account organizes with someone else invited, and the
/// adapter is told to leave the provider's attendee list alone when the edit
/// did not change who is invited.
///
/// `read_calendar` is the calendar the event was read from: on a move to
/// another calendar, the one it leaves. The cache does not hold local events,
/// so for those there is nothing to compare with and the list is written as
/// always.
pub fn guard_update(cache: &CacheStore, account: &str, read_calendar: &str, event: &mut Event) {
    let read = match cache.read_event(account, read_calendar, &event.id) {
        Ok(read) => read,
        Err(err) => {
            tracing::warn!(?err, event_id = %event.id, "reading the cached event for the write guard failed");
            None
        }
    };
    cal_core::attendee::guard_update(event, read.as_ref());
}

/// Before a create: see `cal_core::attendee::guard_create`.
pub fn guard_create(event: &mut NewEvent) {
    cal_core::attendee::guard_create(event);
}
