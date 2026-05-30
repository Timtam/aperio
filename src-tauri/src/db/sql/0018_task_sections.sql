-- Migration 0018: nested projects + sections for tasks.
--
-- Two orthogonal grouping concepts arrive together (DESIGN.md §9 task
-- organisation), mirroring what Vikunja and Todoist expose:
--
--   * task_lists.parent_id — a project can nest under another project.
--     Self-referencing FK; NULL means a top-level list (the only shape
--     the local adapter itself produces, since it's a flat backend —
--     the column exists so synced rows from nesting-capable backends
--     round-trip without loss).
--   * sections                — a Vikunja "bucket" / Todoist "section":
--     an ordered sub-grouping *within* one list. tasks.section_id files
--     a task under one of them; NULL means ungrouped.
--
-- All three columns/tables are additive — no table rebuild needed, so
-- this is a plain ADD COLUMN + CREATE TABLE migration (unlike 0006).

-- Parent project for nested-project backends. ON DELETE SET NULL so
-- removing a parent promotes its children to top-level rather than
-- cascading them away.
ALTER TABLE task_lists
    ADD COLUMN parent_id TEXT REFERENCES task_lists(id) ON DELETE SET NULL;

-- A section groups the tasks of exactly one list. Deleting the list
-- cascades its sections away; `position` drives display order.
CREATE TABLE sections (
    id          TEXT NOT NULL PRIMARY KEY,
    list_id     TEXT NOT NULL REFERENCES task_lists(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX sections_list_id_idx ON sections(list_id);

-- Which section a task is filed under. ON DELETE SET NULL so deleting a
-- section ungroups its tasks rather than deleting them.
ALTER TABLE tasks
    ADD COLUMN section_id TEXT REFERENCES sections(id) ON DELETE SET NULL;

CREATE INDEX tasks_section_id_idx ON tasks(section_id);
