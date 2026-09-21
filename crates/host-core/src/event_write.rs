//! The host's guard in front of every event write (decisions 67a, 70a, 71a,
//! 72a and 106).
//!
//! The rules live in `cal_core::attendee` and `cal_core::event_diff`; this
//! module only feeds them what a host has and a frontend does not: the event as
//! the cache last read it, and whether the cache has been read from the
//! provider since this app last wrote there. Both hosts, the desktop commands
//! and the mobile `Host`, call it before any adapter or the local store sees an
//! event, so neither can forget a path the other covers.

use cal_core::{Event, NewEvent};

use crate::cache::{CacheStore, SyncScope};

/// Before an update. The organizer leaves the invitees, a send intent survives
/// only for an event the account organizes with someone else invited, and the
/// adapter is told to leave the provider's attendee list alone when the edit
/// did not change who is invited.
///
/// `read_calendar` is the calendar the event was read from: on a move to
/// another calendar, the one it leaves. The cache does not hold local events,
/// so for those there is nothing to compare with and the list is written as
/// always.
///
/// And the fields the edit left exactly as that copy has them are marked kept
/// (decision 106), so an adapter that writes field by field leaves them as the
/// provider has them. Only while the calendar has been refreshed since this
/// app last wrote to it: every write only marks the cache stale
/// (`CacheStore::invalidate` clears `last_refreshed_at` and leaves the rows),
/// so until the next refresh the row is the one from BEFORE that write, and an
/// edit rebuilt from it — a split's restore, a revert, a meeting link removed
/// right after it was added — would look untouched field by field and never be
/// written. The stamp is read before the row, so a refresh landing in between
/// can only make the row newer than the stamp says, never older.
pub fn guard_update(cache: &CacheStore, account: &str, read_calendar: &str, event: &mut Event) {
    let refreshed = match cache.get_sync_state(account, SyncScope::Events, read_calendar) {
        Ok(state) => state.is_some_and(|state| state.last_refreshed_at.is_some()),
        Err(err) => {
            tracing::warn!(?err, event_id = %event.id, "reading the calendar's refresh stamp for the write guard failed");
            false
        }
    };
    let read = match cache.read_event(account, read_calendar, &event.id) {
        Ok(read) => read,
        Err(err) => {
            tracing::warn!(?err, event_id = %event.id, "reading the cached event for the write guard failed");
            None
        }
    };
    cal_core::attendee::guard_update(event, read.as_ref());
    // Always assigned, never merged: an event that comes back from a save and
    // is sent again must not carry an old list in.
    event.keep_fields = cal_core::event_diff::kept_fields(event, read.as_ref(), refreshed);
}

/// Before a create: see `cal_core::attendee::guard_create`.
pub fn guard_create(event: &mut NewEvent) {
    cal_core::attendee::guard_create(event);
}
