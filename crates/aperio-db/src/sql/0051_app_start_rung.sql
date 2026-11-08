-- 0051: the "on next app start" reminders this device has already rung.
--
-- Such a reminder rings at the first start of the app after its moment: an
-- event's start (for a series, its latest past occurrence) or a task's due
-- time. Until now nothing remembered that it had, so it rang again at every
-- start for as long as the entry existed. One row per entry and moment; a
-- moved event or a new occurrence is a new moment and rings once more.
--
-- Local to this device and never synced (Toni, 2026-11-08: once per device):
-- "next start" means the next start of this app on this device.
CREATE TABLE app_start_rung (
    item_kind     TEXT NOT NULL,   -- "event" | "task"
    item_id       TEXT NOT NULL,
    reference_utc TEXT NOT NULL,   -- RFC 3339, the moment it rang for
    rung_at       TEXT NOT NULL,   -- RFC 3339
    PRIMARY KEY (item_kind, item_id, reference_utc)
);
