-- Carry the IANA time zone of a recurring event's master DTSTART, so the series
-- expands DST-correctly: a rule authored in winter otherwise drifts an hour (and
-- can slip a day) once it crosses into summer. NULL = no zone (floating / UTC /
-- all-day), which keeps the prior UTC expansion. Synced like the other event
-- columns (it rides the Event payload, so older peers simply ignore it).
ALTER TABLE events ADD COLUMN rrule_tzid TEXT;
