-- Migration 0019: persistent snapshot cache for EXTERNAL adapters.
--
-- Goal: kill the multi-second cold-start latency. External providers
-- (CalDAV, EWS, iCal, Google, Graph, Vikunja, …) are hit over the
-- network on every read today; on a fresh process the adapters'
-- in-memory caches are cold, so the first paint waits on the slowest
-- account. These tables let the host serve the last-known snapshot
-- INSTANTLY at startup (stale-while-revalidate) and refresh in the
-- background.
--
-- Design:
--   * One set of `cache_*` tables, host-owned, adapter-independent.
--     The local adapter's authoritative `source='local'` tables and
--     the event-log applier NEVER touch these — they are a disposable
--     mirror of external provider state, not a source of truth.
--   * Every row carries `account_id` (FK → accounts, ON DELETE CASCADE)
--     so deleting an account wipes its cache for free, and rows from
--     different accounts can never collide on a shared native id.
--   * The full `cal_core` struct is stored as JSON in `payload`. Only
--     the columns we actually query (event start/end for range scans,
--     the container id for scoping/pruning) are broken out. Same
--     forward-compatible philosophy as the SyncEvent payloads: the
--     cache schema does not have to move every time a cal_core struct
--     grows a field, and because it is a CACHE we can drop+rewarm on a
--     shape change rather than write a data migration.
--   * `cache_sync_state` tracks, per (account, scope, container):
--     the delta token / CTag (CACHE-4+ hybrid layer), the covered
--     event window (so we can tell "cached, genuinely empty" from
--     "not cached yet"), and freshness/диagnostics for the
--     "zuletzt aktualisiert" / offline UI.

-- ── Container snapshots ──────────────────────────────────────────────
-- One row per external calendar / task list / contact list. `payload`
-- is the full cal_core container struct as JSON.

CREATE TABLE cache_calendars (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    id          TEXT NOT NULL,
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, id)
);

CREATE TABLE cache_task_lists (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    id          TEXT NOT NULL,
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, id)
);

CREATE TABLE cache_contact_lists (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    id          TEXT NOT NULL,
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, id)
);

-- ── Event snapshots ──────────────────────────────────────────────────
-- start_utc/end_utc are surfaced as columns so the range scan
-- (half-open overlap: start < range.end AND end > range.start) stays
-- index-friendly. etag is surfaced for delta/conditional refresh.
-- Everything else lives in `payload`.

CREATE TABLE cache_events (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    calendar_id TEXT NOT NULL,
    id          TEXT NOT NULL,
    start_utc   TEXT NOT NULL,
    end_utc     TEXT NOT NULL,
    etag        TEXT,
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, calendar_id, id)
);

CREATE INDEX cache_events_range_idx
    ON cache_events(account_id, calendar_id, start_utc, end_utc);

-- ── Task snapshots ───────────────────────────────────────────────────

CREATE TABLE cache_tasks (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    list_id     TEXT NOT NULL,
    id          TEXT NOT NULL,
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, list_id, id)
);

CREATE INDEX cache_tasks_list_idx ON cache_tasks(account_id, list_id);

-- ── Contact snapshots ────────────────────────────────────────────────

CREATE TABLE cache_contacts (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    list_id     TEXT NOT NULL,
    id          TEXT NOT NULL,
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, list_id, id)
);

CREATE INDEX cache_contacts_list_idx ON cache_contacts(account_id, list_id);

-- ── Sync state ───────────────────────────────────────────────────────
-- Per (account, scope, container). `scope` is one of:
--   'calendars' | 'task_lists' | 'contact_lists'  (account-wide listings)
--   'events' | 'tasks' | 'contacts'               (per-container item sets)
-- For account-wide listing scopes `container_id` is '' (empty).
-- `window_start`/`window_end` are only meaningful for the 'events'
-- scope (the cached time window). `sync_token`/`ctag` back the CACHE-4+
-- hybrid delta layer; they are NULL until an adapter that supports
-- incremental sync fills them.

CREATE TABLE cache_sync_state (
    account_id        TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    scope             TEXT NOT NULL,
    container_id      TEXT NOT NULL DEFAULT '',
    sync_token        TEXT,
    ctag              TEXT,
    window_start      TEXT,
    window_end        TEXT,
    last_refreshed_at TEXT,
    last_error        TEXT,
    PRIMARY KEY (account_id, scope, container_id)
);
