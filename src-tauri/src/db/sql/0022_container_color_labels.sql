-- Bind a container's color to a global color-label (DESIGN §6.5 / §8.2).
--
-- Containers (calendars / task lists / address books) get an optional
-- `color_label_id`, mirroring how events + tasks already reference a
-- color label. The rendered color resolves to the label's CURRENT hex,
-- so recoloring the label recolors every bound container. `ON DELETE SET
-- NULL` matches the events/tasks columns: deleting the label clears the
-- binding and the container falls back to its native color.
--
-- For LOCAL containers the binding lives on the row (and syncs with the
-- container's other fields). EXTERNAL containers (Google, CalDAV, …) keep
-- their provider color; the user's binding is a host-local OVERRIDE in
-- `container_color_overrides`, the same shape as `container_name_overrides`.

ALTER TABLE calendars
    ADD COLUMN color_label_id TEXT REFERENCES color_labels(id) ON DELETE SET NULL;

ALTER TABLE task_lists
    ADD COLUMN color_label_id TEXT REFERENCES color_labels(id) ON DELETE SET NULL;

ALTER TABLE contact_lists
    ADD COLUMN color_label_id TEXT REFERENCES color_labels(id) ON DELETE SET NULL;

CREATE TABLE container_color_overrides (
    container_id    TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN ('calendar', 'task_list', 'contact_list')),
    -- The bound label. CASCADE so a deleted label drops the override and
    -- the external container reverts to its provider color.
    color_label_id  TEXT NOT NULL REFERENCES color_labels(id) ON DELETE CASCADE,
    updated_at      TEXT NOT NULL,
    PRIMARY KEY (container_id, kind)
);

CREATE INDEX container_color_overrides_kind_idx
    ON container_color_overrides(kind);
