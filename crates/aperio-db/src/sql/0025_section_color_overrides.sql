-- Migration 0025: local color-label override for EXTERNAL task sections
-- (Todoist sections, Vikunja kanban buckets), which carry no provider
-- color field of their own. Mirrors `container_color_overrides`
-- (migration 0022), but sections are a single id namespace so there is no
-- `kind` column.
--
-- LOCAL sections store their colour binding directly on
-- `sections.color_label_id` (migration 0024) and never appear here: a
-- section is local or external for its whole life (its owning list's
-- account never changes), so a single `section_id` is never both a synced
-- local binding and an override row.
CREATE TABLE section_color_overrides (
    section_id      TEXT NOT NULL PRIMARY KEY,
    color_label_id  TEXT NOT NULL REFERENCES color_labels(id) ON DELETE CASCADE,
    updated_at      TEXT NOT NULL
);
