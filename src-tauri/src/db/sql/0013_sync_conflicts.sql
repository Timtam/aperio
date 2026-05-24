-- Migration 0013: per-field conflict log for the event-log applier
-- (DESIGN.md §19.3, Phase Sh).
--
-- Two devices that edit the same field of the same row on
-- divergent timelines generate a conflict the applier can't
-- auto-merge. We record both candidate values here so the
-- frontend can offer the §19.3 resolution dialog ("Meine Version
-- behalten" / "Andere Version nehmen" / "Beide als separate
-- Termine speichern").
--
-- One row per (row_kind, row_id, field) — a fresh divergence on
-- the same field overwrites the prior unresolved record, because
-- the user only needs to resolve the most recent state. We
-- enforce that via a partial UNIQUE INDEX on the unresolved
-- subset, so resolved history rows stay around for audit but
-- don't block new conflicts.
--
-- Resolution rules:
--
--   - `keep_local`:  no data change; flip resolved=1.
--   - `take_remote`: write remote_value back into the row +
--                    emit a new SyncEvent::*Updated so other
--                    devices converge. Flip resolved=1.
--   - `save_both`:   create a copy of the row with all fields
--                    cloned + the conflicting field replaced
--                    with remote_value. The original keeps the
--                    local value. Both rows now exist as
--                    independent entities.
--
-- All values are JSON-encoded so they round-trip through the
-- same serde paths the wire format uses. The `row_kind`
-- discriminator is one of: "event", "task", "task_list",
-- "calendar", "color_label". Anything else is a hostile or
-- buggy peer; the applier rejects it before it lands here.

CREATE TABLE sync_conflicts (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    detected_at         TEXT NOT NULL,
    row_kind            TEXT NOT NULL,
    row_id              TEXT NOT NULL,
    field               TEXT NOT NULL,
    -- Local and remote values as JSON strings. Either may be
    -- NULL if the field was previously unset on that side.
    local_value         TEXT,
    remote_value        TEXT,
    -- Originator of the remote edit. Used in the resolution
    -- dialog to surface "Geändert vor 3 Min auf <Gerätename>".
    remote_device_id    TEXT NOT NULL,
    remote_timestamp    TEXT NOT NULL,
    -- Resolution flag. 0 = unresolved (user action required);
    -- 1 = resolved (kept for audit; cleared on a later sweep
    -- if we ever add retention).
    resolved            INTEGER NOT NULL DEFAULT 0,
    resolution          TEXT,
    resolved_at         TEXT
);

-- Only one unresolved row per (kind, id, field). A second
-- conflict on the same field while the first is pending
-- supersedes it — the user resolves once.
CREATE UNIQUE INDEX idx_sync_conflicts_unresolved_unique
    ON sync_conflicts(row_kind, row_id, field)
    WHERE resolved = 0;

-- Fast lookup for the "show me the pending conflicts" command.
CREATE INDEX idx_sync_conflicts_unresolved_recent
    ON sync_conflicts(resolved, detected_at DESC);
