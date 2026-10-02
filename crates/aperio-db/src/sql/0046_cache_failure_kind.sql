-- Why a container's refresh fails, and when it last succeeded.
--
-- `failure_kind` records what the error WAS, not what its text looks like:
-- 'access' (the operating system has not granted the device's calendars or
-- reminders), 'auth' (a login problem), or NULL for anything else. Until
-- now the error surface guessed "auth" from words in the message, and a
-- withheld OS grant read as a store still loading: every read retried it
-- at once, in bursts, forever. With the kind on record, the cache stops
-- retrying an account the OS has not opened, and the surface says why.
-- Read only together with `last_error`, which every success clears.
--
-- `last_success_at` keeps the last successful refresh across the resets
-- that clear `last_refreshed_at` (a manual full re-sync, an invalidation):
-- until now a reset made the surface say "never updated successfully".
-- NULL means "the last success is `last_refreshed_at`".
ALTER TABLE cache_sync_state ADD COLUMN failure_kind TEXT;
ALTER TABLE cache_sync_state ADD COLUMN last_success_at TEXT;
