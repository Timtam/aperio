-- The iCalendar UID an event was imported with (a .ics file, DESIGN-file-import.md
-- I4), so importing the same file again finds it. NULL for an event made in
-- Aperio, which has no UID of its own to keep. Never changed after the
-- create. Synced like the other event columns: it rides the Event payload, and
-- a device's update that does not carry it keeps the stored one.
ALTER TABLE events ADD COLUMN ical_uid TEXT;
