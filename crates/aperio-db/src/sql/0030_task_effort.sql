-- Task effort estimate (DESIGN: an Aperio-only "Aufwand" field, modelled
-- exactly like `priority`).
--
-- `effort` is a three-level estimate ("small" | "medium" | "large") with no
-- native home on any external provider, so — like `priority` — it rides the
-- existing `tasks.*` event log as an ordinary column on a LOCAL list, and the
-- AperioExtras bag (extras.rs) on an external list. It drives a purely visual,
-- toggleable tile size in the UI.
--
-- NOT NULL DEFAULT 'medium' so the column backfills every existing row to the
-- neutral middle without a data migration (the table is already populated, so
-- a plain NOT NULL with no default would fail the ALTER).

ALTER TABLE tasks
    ADD COLUMN effort TEXT NOT NULL DEFAULT 'medium';  -- "small" | "medium" | "large"
