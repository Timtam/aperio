-- DESIGN.md §19 device registry — local cache of every
-- device's human-readable name from the cross-device
-- `meta.json`'s `devices` map.
--
-- The orchestrator upserts into this table after every
-- successful `fetch_meta` round so the rest of the host
-- (currently: the §20.8 "Plugin benötigt" panel) can render
-- device names without a network round-trip + without
-- coupling to the orchestrator's lifecycle.
--
-- One row per device id. Names can be missing (DeviceRecord's
-- `name` field is optional) — the row still gets written so a
-- later upsert can fill it in.

CREATE TABLE device_names (
    device_id TEXT NOT NULL PRIMARY KEY,
    name      TEXT
);
