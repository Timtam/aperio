-- Migration 0026: host-local color-label override for EXTERNAL calendar
-- events whose provider can't store a per-event color (notably iCloud, and
-- any CalDAV server / account that doesn't support RFC 7986 COLOR, plus
-- Graph / EWS). Mirrors `section_color_overrides` (migration 0025): a single
-- id namespace, so no `kind` column.
--
-- LOCAL events store their color binding directly on `events.color_label_id`
-- (migration 0001), and external events on color-capable calendars get their
-- color written to / read from the provider (RFC 7986). Those never appear
-- here — an event is local or external for its whole life (its calendar's
-- account never changes), and a color-capable calendar carries the binding
-- on the provider, so a single `event_id` is never both a provider binding
-- and an override row.
--
-- The id is the series master id (recurrence lives on the master); the color
-- applies to the whole series, matching RFC 7986 COLOR on the VEVENT.
CREATE TABLE event_color_overrides (
    event_id        TEXT NOT NULL PRIMARY KEY,
    color_label_id  TEXT NOT NULL REFERENCES color_labels(id) ON DELETE CASCADE,
    updated_at      TEXT NOT NULL
);
