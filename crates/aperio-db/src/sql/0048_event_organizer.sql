-- The organizer of a meeting someone else organizes, imported from a file into
-- a local calendar (DESIGN-file-import.md, I6): the invitation is kept as it
-- is, with its invitees in `attendees`. NULL for every appointment made in
-- Aperio, which the account organizes itself. Never changed after the create.
-- Synced like the other event columns: it rides the Event payload, and a
-- device's update that does not carry it keeps the stored one.
ALTER TABLE events ADD COLUMN organizer TEXT;
