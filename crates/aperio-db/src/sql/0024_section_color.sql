-- Bind a task section's color to a global color-label (DESIGN §9).
--
-- Sections get an optional `color_label_id`, mirroring how tasks and task
-- lists already reference a color label. The rendered color resolves to
-- the label's CURRENT hex, so recoloring the label recolors every bound
-- section live. The color cascades to the section's tasks that carry no
-- color of their own (resolution chain task -> section -> list).
-- `ON DELETE SET NULL` matches the events/tasks/task_lists columns:
-- deleting the label clears the binding and the section falls back to its
-- list's color.
--
-- Sections are a purely local, Aperio-synced concept (no provider
-- round-trip), so the binding lives on the row and rides the existing
-- `section.*` event log — no override table is needed (unlike external
-- containers, whose binding lives in `container_color_overrides`).

ALTER TABLE sections
    ADD COLUMN color_label_id TEXT REFERENCES color_labels(id) ON DELETE SET NULL;
