-- Migration 0014: per-round sync log for the §19.9 "Detailliertes
-- Sync-Protokoll" surface.
--
-- The scheduler appends one row per attempted sync round; the
-- Settings → Synchronisation → Protokoll list reads from here.
-- Each row carries enough to render either "synced N events" or
-- "failed: <reason>" without re-deriving anything from the live
-- orchestrator state.
--
-- ## Retention
--
-- The intent is "recent history for diagnosis", not an audit
-- log. The repo prunes to the most recent ~200 rows after every
-- insert; even at one round per minute that's ~3 hours of
-- history, which comfortably covers the "what was happening
-- before I went to bed" use case the §19.9 spec implies.
-- 200 rows × ~300 bytes ≈ 60 kB on disk, negligible.
--
-- ## Trigger taxonomy
--
-- The `trigger` column records WHY this round ran. Values:
--
--   - `app_start`  — initial sync after `APP_START_DELAY`
--   - `periodic`   — `tokio::time::sleep(interval)` woke us
--   - `kick`       — the EventLogWriter's debounced notify
--   - `manual`     — user clicked "Sync now"
--   - `app_exit`   — final push on `RunEvent::ExitRequested`
--
-- This is purely diagnostic; the UI groups rows by date but
-- doesn't filter on trigger. A future "show me only manual
-- runs" filter would be a one-liner if anyone asks.
--
-- ## Round outcome
--
-- A round either Succeeds (success = 1) or Fails (success = 0).
-- Success rows carry the four counter fields populated; failure
-- rows carry the error message + null counters. We deliberately
-- store the counters AND the error so partial-progress rounds
-- (e.g. push phase succeeded, fetch phase failed) can still
-- report what was pushed.

CREATE TABLE sync_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    recorded_at     TEXT NOT NULL,
    trigger         TEXT NOT NULL,
    success         INTEGER NOT NULL,
    -- Counters from SyncRoundReport. NULL when the round
    -- failed before any phase produced numbers.
    pushed_logs     INTEGER,
    fetched_logs    INTEGER,
    applied         INTEGER,
    conflicts       INTEGER,
    -- Wall-clock duration in milliseconds. NULL on rounds
    -- that aborted before the duration could be measured.
    duration_ms     INTEGER,
    -- Set when `success = 0`. Free-form error string from the
    -- orchestrator; the UI just renders it verbatim.
    error           TEXT
);

-- Newest-first reads — the Protokoll list shows recent rounds at
-- the top. The PK is monotonically increasing AND `recorded_at`
-- is ISO-8601 lexicographic, so either would work; indexing the
-- timestamp lets a future "last 24 h" filter use a range scan.
CREATE INDEX idx_sync_log_recorded_at_desc ON sync_log(recorded_at DESC);
