//! The host's guard in front of every event write (decisions 67a, 70a, 71a,
//! 72a and 106).
//!
//! The rules live in `cal_core::attendee` and `cal_core::event_diff`; this
//! module only feeds them what a host has and a frontend does not: the event as
//! the cache last read it, and whether the cache has been read from the
//! provider since this app last wrote there — which the ticket it hands out
//! keeps true while the write is in flight. Both hosts, the desktop commands
//! and the mobile `Host`, call it before any adapter or the local store sees an
//! event, so neither can forget a path the other covers.

use cal_core::{Event, NewEvent};

use crate::cache::CacheStore;

/// An event write in progress, from the guard until the host drops it
/// (decision 106). While it is held, and after it ends however it ends, no
/// cached row of that calendar counts as the copy an editor opened until a
/// refresh has read the calendar again ([`CacheStore::events_proven`]). Hold
/// it across the adapter call: `let _write = guard_update(..)`. `let _ = ..`
/// would drop it at once.
#[must_use = "the write is fenced only while the ticket is held"]
pub struct EventWriteTicket {
    cache: CacheStore,
    account: String,
    calendar: String,
}

impl Drop for EventWriteTicket {
    fn drop(&mut self) {
        self.cache.end_event_write(&self.account, &self.calendar);
    }
}

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
/// provider has them — only while every cached row of the calendar was read
/// from the provider after this app's last write there
/// ([`CacheStore::events_proven`]). A write only marks the cache stale and
/// leaves the rows, so until the next refresh a row is the one from BEFORE that
/// write, and an edit rebuilt from it — a split's restore, a revert, a meeting
/// link removed right after it was added — would look untouched field by field
/// and never be written. The proof is asked before the row is read, so a
/// refresh landing in between can only make the row newer, never older.
///
/// Returns the write's ticket: the host holds it across the adapter call.
pub fn guard_update(
    cache: &CacheStore,
    account: &str,
    read_calendar: &str,
    event: &mut Event,
) -> EventWriteTicket {
    let proven = cache.events_proven(account, read_calendar);
    let read = match cache.read_event(account, read_calendar, &event.id) {
        Ok(read) => read,
        Err(err) => {
            tracing::warn!(?err, event_id = %event.id, "reading the cached event for the write guard failed");
            None
        }
    };
    cal_core::attendee::guard_update(event, read.as_ref(), proven);
    // Always assigned, never merged: an event that comes back from a save and
    // is sent again must not carry an old list in.
    event.keep_fields = cal_core::event_diff::kept_fields(event, read.as_ref(), proven);
    // Begun only now, so this write's own ticket never disproves the copy it
    // was just compared with.
    cache.begin_event_write(account, read_calendar);
    EventWriteTicket {
        cache: cache.clone(),
        account: account.to_string(),
        calendar: read_calendar.to_string(),
    }
}

/// Before a create: see `cal_core::attendee::guard_create`.
pub fn guard_create(event: &mut NewEvent) {
    cal_core::attendee::guard_create(event);
}
