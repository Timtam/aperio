-- One-time re-warm of the external-event snapshot cache.
--
-- Before this fix a recurring master's `cache_events.end_utc` column held
-- its FIRST occurrence's end, so a long-running series (e.g. an iCloud
-- weekly meeting whose DTSTART is a year in the past) was filtered out of
-- the half-open range query in `read_events` and never reached the
-- frontend to be expanded — its occurrences simply didn't show in the
-- month view. `insert_event` now stores the recurrence's reach (the
-- parsed `UNTIL`, or a far-future sentinel for open-ended / COUNT-based
-- series) instead.
--
-- Rows written before the fix still carry the old value, and a delta sync
-- won't re-send an unchanged master, so clearing the cached events plus
-- their sync state forces the next view load to do a full re-fetch that
-- re-inserts every master with the corrected `end_utc`. The cache is
-- disposable (stale-while-revalidate), so the only cost is one refresh.
DELETE FROM cache_events;
DELETE FROM cache_sync_state WHERE scope = 'events';
