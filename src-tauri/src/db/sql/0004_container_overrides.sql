-- Phase 6c-follow-up: local renames for any calendar / task list.
--
-- The frontend lets the user override the display name of any
-- container regardless of its source. For local and CalDAV containers
-- a future migration will also push the rename to the source server
-- (PROPPATCH DAV:displayname, UPDATE calendars.name, ...), but for
-- read-only sources (iCal feeds, public shares) the override is the
-- only place the new name can live.
--
-- Schema notes:
--   - `kind` discriminates between calendar and task-list ids. The two
--     namespaces are disjoint today, so a single primary key would
--     work, but keeping `kind` explicit keeps the foreign frontend
--     surface obvious and lets a future code-path enforce "only
--     calendars" or "only task lists" cheaply.
--   - No foreign key into `calendars` / `task_lists` because external
--     ids (CalDAV collection URLs, iCal SHA prefixes) don't appear
--     in those tables — only local ids would.
--   - `name` is the override; an empty string is reserved (caller
--     should DELETE the row to revert to the source name).

CREATE TABLE container_name_overrides (
    container_id    TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('calendar', 'task_list')),
    name            TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (container_id, kind)
);

CREATE INDEX container_name_overrides_kind_idx
    ON container_name_overrides(kind);
