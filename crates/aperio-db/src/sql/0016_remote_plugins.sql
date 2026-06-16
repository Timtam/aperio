-- DESIGN.md §20.8 — plugin sync across devices.
--
-- When another device installs / updates / uninstalls a
-- community plugin, the event log carries the metadata (NOT
-- the binary). This table mirrors what we've learned from
-- those events so the Settings → Plugins panel can render the
-- "Plugin benötigt" section + the AccountsPanel can mark
-- account rows whose plugin id we don't have installed
-- locally.
--
-- One row per plugin id. Bundled plugins never appear here
-- (they're guaranteed present on every install + filtered out
-- at write time on the announcing side). Reinstalls land on
-- the same row via ON CONFLICT DO UPDATE.

CREATE TABLE remote_plugins (
    -- Plugin id (reverse-DNS, matches the manifest's `id` +
    -- the local PluginManager's keying). Primary key — two
    -- different devices announcing the same id collapse to
    -- one row with the most recent metadata winning.
    id                     TEXT NOT NULL PRIMARY KEY,

    -- Human-readable name from the announcing device's
    -- manifest. Optional because pre-iteration-21 Aperios
    -- didn't include `name` in the PluginPayload; the row
    -- falls back to the bare id on the UI side.
    name                   TEXT,

    -- Plugin version as announced by the most recent
    -- `plugin.installed` / `plugin.updated` event.
    version                TEXT NOT NULL,

    -- Plugin-type wire string (`calendar-adapter`,
    -- `sync-adapter`, etc.). Optional for the same backward-
    -- compat reason as `name`.
    plugin_type            TEXT,

    -- Optional distribution source from the announcing event
    -- (registry URL, download link, …). The §20.8 dialog
    -- shows it to the user when present so they know where
    -- to get the binary.
    source                 TEXT,

    -- The device id that emitted the most recent event for
    -- this plugin. Useful for the "this plugin is used on
    -- your <DeviceName>" hint in the UI.
    announced_by_device    TEXT NOT NULL,

    -- RFC 3339 timestamp of the most recent announcing
    -- event. Lets the UI sort the missing-plugins list "most
    -- recently announced first" so freshly-installed
    -- plugins on other devices float up.
    announced_at           TEXT NOT NULL
);

-- Listing-side index: the UI typically fetches "all rows
-- ordered by announced_at DESC" so the missing-plugins list
-- keeps the most relevant entries at the top without an
-- ORDER BY full scan.
CREATE INDEX idx_remote_plugins_announced_at
    ON remote_plugins(announced_at DESC);
