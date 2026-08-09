-- Migration 0035: which events mean the same appointment.
--
-- The same commitment routinely exists several times over: once in the work
-- calendar so colleagues see it, once copied into a private calendar because
-- that is the one a voice assistant reads out, and again in each colleague's
-- calendar that Aperio also reads. To every provider these are unrelated
-- events, and until now they were unrelated to Aperio too — several identical
-- rows in a day, several places to fix when the time moves, and a meeting link
-- attached to whichever of them happened to be in front of the user.
--
-- A group is a statement Aperio makes ABOUT foreign data: these events mean the
-- same appointment. It is not an event, it replaces none, and no provider ever
-- learns of it. See DESIGN-event-groups.md.
--
-- SYNCED, unlike `event_meetings` (migration 0034), and for the opposite
-- reason. A meeting id is bookkeeping about a provider object that every device
-- can already reach through the event itself. A grouping exists nowhere but
-- here: a phone that has not been told stays convinced it is looking at four
-- separate appointments. The knowledge has no other route between devices, so
-- it takes ours.
--
-- Membership carries a SIGNATURE — the title and start it had when it joined —
-- and not only the ids. Event ids belong to the provider and change underneath
-- us: a re-bootstrap remints them, moving an event between calendars remints
-- it, and Exchange does it unprompted. A group that stored ids alone would lose
-- limbs in silence, which is the worst of the available failures, because a
-- group that is quietly incomplete still looks authoritative. With a signature
-- a lost member can be found again, and one that cannot be found can be
-- reported instead of dropped.
--
-- `event_id` is the SERIES MASTER id, matching `event_meetings` and
-- `event_color_overrides`: a recurring appointment is grouped as a series, not
-- one row per occurrence. Grouping single occurrences of a series is a
-- different feature and deliberately not this one.
CREATE TABLE event_groups (
    id          TEXT NOT NULL PRIMARY KEY,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE TABLE event_group_members (
    group_id     TEXT NOT NULL REFERENCES event_groups(id) ON DELETE CASCADE,
    calendar_id  TEXT NOT NULL,
    event_id     TEXT NOT NULL,
    -- The signature, as of joining. Kept for re-finding a member whose id the
    -- provider has changed, never for display: what the user reads always comes
    -- from the event itself, which may since have been renamed.
    title        TEXT NOT NULL,
    starts_at    TEXT NOT NULL,
    added_at     TEXT NOT NULL,
    PRIMARY KEY (group_id, calendar_id, event_id)
);

-- An event belongs to at most ONE group. Without this, "which group does this
-- row collapse into" has no answer, and two groups could each claim to be the
-- whole truth about the same appointment.
CREATE UNIQUE INDEX event_group_members_event_uidx
    ON event_group_members(calendar_id, event_id);

-- The lookup every calendar view makes: given the events of a day, which of
-- them are grouped.
CREATE INDEX event_group_members_group_idx ON event_group_members(group_id);
