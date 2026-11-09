-- 0052: re-warm the external event snapshots after an iCalendar read change.
--
-- The iCalendar reader now reads two alarms it read differently before:
-- `TRIGGER;RELATED=END` is a reminder before the END (`relative_to_end`;
-- before, it read as the same minutes before the START, and an e-mail alarm
-- from the end as an e-mail reminder from the start), and a duration in weeks
-- (Apple's "1 week before", `-P1W`) is seven days before (before, it was not
-- read at all). The cached copies still carry the old reading, and a save of
-- one would hand the server reminders that no longer match its alarms: the
-- write claims the server's alarms by the new reading, so Apple's week alarm
-- would go, and an alarm before the end would turn into one before the start.
-- A delta sync does not re-send an unchanged appointment, so clearing the
-- cached events plus their sync state forces one full re-fetch, as 0050 did.
-- The cache is disposable (stale-while-revalidate); the cost is one refresh.
DELETE FROM cache_events;
DELETE FROM cache_sync_state WHERE scope = 'events';
