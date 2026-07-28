-- Migration 0034: which provider-side meeting Aperio created for which event.
--
-- The videoconference adapters mint a meeting and hand back an opaque
-- provider id. The join URL goes into the event itself — into a field the
-- provider stores and every other calendar client can read, which is what
-- makes a meeting reachable from Outlook, from eM Client and from a phone that
-- has never heard of Aperio. But the join URL is not the meeting id: Webex's
-- link carries an `MTID` and nothing that identifies the meeting to its own
-- API. Without a record here, a meeting Aperio created could be joined forever
-- and never deleted.
--
-- HOST-LOCAL, deliberately, and never written to the sync log:
--
--   * It is bookkeeping about a provider object, not part of the event. The
--     event already carries everything a user needs (the link), and every
--     device — including devices running other apps — reads it from there.
--   * The consequence is honest and bounded: the device that created a meeting
--     can delete it with its event; another device deletes only the event and
--     leaves the meeting standing on the provider, where the user can remove
--     it. Making it synced would mean putting a provider-object id on the wire
--     to save a cleanup that the provider's own web UI already offers.
--
-- `event_id` is the SERIES MASTER id, matching `event_color_overrides`
-- (migration 0026): one meeting serves a whole recurring series, exactly as a
-- recurring meeting does on the provider side.
--
-- The account reference is by id and NOT a foreign key: deleting the Webex
-- account should not silently drop the record of meetings it created, because
-- the row is also what tells the UI "this event's meeting was made by an
-- account you no longer have". Rows are cleaned up when their event goes.
CREATE TABLE event_meetings (
    event_id    TEXT NOT NULL PRIMARY KEY,
    account_id  TEXT NOT NULL,
    meeting_id  TEXT NOT NULL,
    join_url    TEXT NOT NULL,
    created_at  TEXT NOT NULL
);

CREATE INDEX event_meetings_account_idx ON event_meetings(account_id);
