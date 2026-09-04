-- Migration 0043: reminders Aperio keeps for one event and tells nobody about.
--
-- A reminder on an event normally rides ON the event: Aperio writes it as a
-- VALARM (or the provider's equivalent), the provider stores it, and every
-- other client of that calendar rings too — the iOS Calendar app, a voice
-- assistant reading the account out loud. That is what makes an appointment
-- announced everywhere, and it is what most reminders should be.
--
-- Sometimes it is exactly what the user does NOT want. A shared work calendar
-- is not the place to announce "leave now" to the whole team, and a colleague
-- reading the same iCloud calendar has no business being reminded of it. Such
-- a reminder has nowhere to live on the provider: there is no field for "this
-- alarm is mine alone". So it lives here.
--
-- SYNCED, like `event_groups` (migration 0035) and for the same reason: the
-- knowledge exists nowhere but in Aperio, so our own sync is its only route
-- between the user's devices. A phone that has not been told simply would not
-- ring — which is the whole point of the record. It reaches the user's OTHER
-- Aperio devices and stops there: another person on the same shared calendar
-- never learns of it, because it never touches the provider.
--
-- Contrast `event_color_overrides` (migration 0026) and `event_meetings`
-- (0034), which are host-local: a colour is a rendering choice a device can
-- make again, and a meeting id is bookkeeping about an object every device can
-- reach through the event itself. A reminder that does not fire is not a
-- degraded rendering; it is a missed appointment.
--
-- Membership carries a SIGNATURE — the title and start the event had when the
-- reminder was set — for the reason spelled out in migration 0035: event ids
-- belong to the provider and change underneath us. A re-bootstrap remints
-- them, moving an event between calendars remints it, and Exchange bakes a
-- change token into its ids and remints them unprompted. A row that stored the
-- id alone would go quiet in silence, and a reminder that silently stops
-- ringing is the worst failure available here. With a signature the event can
-- be found again and the row repointed.
--
-- `event_id` is the SERIES MASTER id, matching `event_groups`,
-- `event_color_overrides` and `event_meetings`: a recurring appointment is
-- reminded of as a series, exactly as its own reminders are.
--
-- An emptied list is stored as '[]' rather than deleting the row. The row is
-- the record of a decision ("no reminder of my own here"), and deleting it
-- would let a peer that had not yet heard re-assert the old list on the next
-- round — the last writer has to have something to win against.
CREATE TABLE event_local_reminders (
    calendar_id TEXT NOT NULL,
    event_id    TEXT NOT NULL,
    reminders   TEXT NOT NULL,
    title       TEXT NOT NULL,
    starts_at   TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (calendar_id, event_id)
);
