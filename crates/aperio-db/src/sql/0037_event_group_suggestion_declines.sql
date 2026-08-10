-- Migration 0037: "these two are NOT the same appointment."
--
-- Aperio can recognise a copy — same name, same start, another calendar — and
-- the design's chosen shape is to SUGGEST it: recognised, offered, confirmed
-- once, then remembered. This table is the remembering, and specifically the
-- remembering of NO.
--
-- Without it the suggestion is a nag. Two colleagues' "Team meeting" at 10:00
-- in two calendars look exactly like a copy of one appointment and are not;
-- told once, Aperio has to stop asking, or the feature costs more attention
-- than it saves — every single day, for a screen-reader user one more row to
-- walk past every morning.
--
-- The pair is stored in a CANONICAL order (the smaller (calendar, event) pair
-- first, compared as text), so "A and B" and "B and A" are one row and one
-- decision. Without that the same pair could be declined twice and re-offered
-- from the other side.
--
-- Ids are the SERIES MASTER's, like every other event reference Aperio keeps.
--
-- SYNCED, and trivially so: this is a set that only ever grows, so two devices
-- declining different pairs converge by union and there is nothing for a
-- last-writer rule to decide. That is a deliberate contrast with the groups
-- themselves (migrations 0035/0036), where membership can change in both
-- directions and the ordering rules had to be worked out carefully.
--
-- A decline is not a statement that the events are unrelated forever: grouping
-- them by hand still works, and does not consult this table. It only silences
-- the OFFER.
CREATE TABLE event_group_suggestion_declines (
    calendar_a   TEXT NOT NULL,
    event_a      TEXT NOT NULL,
    calendar_b   TEXT NOT NULL,
    event_b      TEXT NOT NULL,
    declined_at  TEXT NOT NULL,
    PRIMARY KEY (calendar_a, event_a, calendar_b, event_b)
);
