-- 0050: re-warm the external event snapshots after an iCalendar read change.
--
-- The iCalendar reader stopped turning alarms it does not model into
-- reminders: an ACTION other than DISPLAY, AUDIO or EMAIL (RFC 5545 §3.8.6.1
-- says to ignore those), and any alarm at the trigger RFC 9074 §8 sets aside
-- for "ignore me". Apple's iPhone writes such an alarm into every appointment
-- it makes, and the cached copies of those appointments still carry it as a
-- reminder at 1976-04-01 00:55:45 UTC: the editors show it, and a save of an
-- untouched cached copy would write it back as an alarm the iPhone shows.
-- A delta sync does not re-send an unchanged appointment, so clearing the
-- cached events plus their sync state forces one full re-fetch, as 0021 did.
-- The cache is disposable (stale-while-revalidate); the cost is one refresh.
DELETE FROM cache_events;
DELETE FROM cache_sync_state WHERE scope = 'events';
