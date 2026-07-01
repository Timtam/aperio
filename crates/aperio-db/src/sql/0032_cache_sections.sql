-- Migration 0032: snapshot cache for EXTERNAL task-list SECTIONS.
--
-- Sections (Vikunja buckets / Todoist sections) of an external list were
-- the last per-container read still doing an UNCONDITIONAL live provider
-- fetch on every day/week-view load (cal-ffi `sections_json`). This table
-- mirrors `cache_tasks` exactly, storing one serialized `cal_core::Section`
-- per (account, list_id, id), so the stale-while-revalidate read path can
-- serve sections from the snapshot when warm and self-warm when cold/stale
-- — identical to how external tasks are already cached (migration 0019).
--
-- Additive new table only; nothing existing is altered. The `sections`
-- sync-scope reuses the shared `cache_sync_state` table (its `scope` column
-- is free-form text), so no schema change is needed there.

CREATE TABLE cache_sections (
    account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    list_id     TEXT NOT NULL,
    id          TEXT NOT NULL,
    native_id   TEXT NOT NULL DEFAULT '',
    payload     TEXT NOT NULL,
    cached_at   TEXT NOT NULL,
    PRIMARY KEY (account_id, list_id, id)
);

CREATE INDEX cache_sections_list_idx ON cache_sections(account_id, list_id);
