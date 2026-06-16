-- Generic key/value store for user preferences.
--
-- Started for the sidebar's tree-expansion state (Phase 6 follow-up)
-- but designed as a catch-all so future "remember this setting"
-- features don't each need their own table.
--
-- Schema notes:
--   - `key` is the namespaced setting id (e.g. `sidebar.expansion`,
--     `view.last_used`, …). Apps that want to scope settings per
--     account / per profile encode that into the key.
--   - `value` is opaque to the table; callers serialise JSON if
--     they need structure. Stored as TEXT for portability — SQLite
--     blobs would be marginally smaller but harder to debug.
--   - `updated_at` is RFC 3339 UTC; useful when sync adapters land
--     because they need a last-write-wins comparison without
--     loading the value.
--
-- Sync compatibility: future sync-adapter migrations can extend
-- this table with an `etag` / `vector_clock` column without breaking
-- the read API; both `key` and `value` stay stable forever.

CREATE TABLE user_prefs (
    key             TEXT PRIMARY KEY,
    value           TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);
