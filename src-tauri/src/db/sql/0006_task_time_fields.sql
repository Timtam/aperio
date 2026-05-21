-- Migration 0006: Merge "Deadline (on)" into scheduled_date, add scheduled_time,
-- make deadline_time optional, drop deadline_type column.
--
-- BEFORE:
--   scheduled_date  -- "Geplanter Tag"
--   deadline_type   -- "on" (must be on this day) | "by" (must be done by this day)
--   deadline_date
--   deadline_time   -- only meaningful for type='on'
--
-- AFTER:
--   scheduled_date  -- the day the task is to be done (was: scheduled OR deadline_type='on')
--   scheduled_time  -- optional time-of-day on that day (point marker, no duration)
--   deadline_date   -- the day by which it must be done (was: deadline_type='by')
--   deadline_time   -- optional time on the deadline day
--
-- Migration rules for existing rows:
--   * deadline_type IS NULL                 → no change to the date/time fields
--   * deadline_type = 'by'                  → keep deadline_date/_time as is
--                                             ("by" is now the only deadline semantic)
--   * deadline_type = 'on', scheduled NULL  → move deadline_date/_time over to
--                                             scheduled_date/scheduled_time
--   * deadline_type = 'on', scheduled set,  → keep both: scheduled_date stays put,
--     scheduled_date != deadline_date         deadline_date becomes the new
--                                             "by" deadline (Plan + Soft-Deadline)
--   * deadline_type = 'on', scheduled set,  → scheduled stays, deadline_date/_time
--     same day                                cleared (they were redundant duplicates)
--
-- SQLite can't drop a column on a pre-3.35 storage format and CHECK constraints
-- can only be added on table create, so this is a full create-new + copy-data +
-- drop-old + rename dance inside the migration transaction.
--
-- Trigger choreography: migration 0002 attaches `tasks_fts_ai/ad/au` triggers
-- to the `tasks` table for FTS sync. Dropping the table also drops those
-- triggers; we have to recreate them at the end of this migration so search
-- keeps working. `task_lists_fts_rename` and `color_labels_fts_rename` are
-- attached to OTHER tables but reference `tasks` in their bodies — SQLite
-- validates their references at DROP TABLE time, so we have to drop and
-- recreate them too. All recreated trigger bodies match 0002 verbatim.

-- 0. Drop every trigger that references the tasks table — either directly
-- (attached to tasks) or via body subqueries — so the table swap below
-- doesn't trip SQLite's referential-integrity checks.
DROP TRIGGER IF EXISTS tasks_fts_ai;
DROP TRIGGER IF EXISTS tasks_fts_ad;
DROP TRIGGER IF EXISTS tasks_fts_au;
DROP TRIGGER IF EXISTS task_lists_fts_rename;
DROP TRIGGER IF EXISTS color_labels_fts_rename;

-- 1. Build the new tasks table next to the old one. Same columns as before
-- minus `deadline_type`, plus `scheduled_time`, with CHECK constraints
-- tying _time to _date.
CREATE TABLE tasks_new (
    id              TEXT NOT NULL PRIMARY KEY,
    list_id         TEXT NOT NULL REFERENCES task_lists(id) ON DELETE CASCADE,
    parent_id       TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    description     TEXT,
    status          TEXT NOT NULL,
    priority        TEXT NOT NULL,
    scheduled_date  TEXT,                                       -- ISO 8601 date
    scheduled_time  TEXT,                                       -- HH:MM:SS, only with scheduled_date
    deadline_date   TEXT,                                       -- ISO 8601 date ("by")
    deadline_time   TEXT,                                       -- HH:MM:SS, only with deadline_date
    recurrence      TEXT,
    color_label_id  TEXT REFERENCES color_labels(id) ON DELETE SET NULL,
    reminders       TEXT NOT NULL DEFAULT '[]',
    sound           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    completed_at    TEXT,
    etag            TEXT,
    CHECK (scheduled_time IS NULL OR scheduled_date IS NOT NULL),
    CHECK (deadline_time  IS NULL OR deadline_date  IS NOT NULL)
);

-- 2. Copy data over, applying the migration rules in the header. Each CASE
-- chooses whether a column should be inherited verbatim, replaced with the
-- old deadline_*, or cleared.
INSERT INTO tasks_new (
    id, list_id, parent_id, title, description, status, priority,
    scheduled_date, scheduled_time, deadline_date, deadline_time,
    recurrence, color_label_id, reminders, sound,
    created_at, updated_at, completed_at, etag
)
SELECT
    id, list_id, parent_id, title, description, status, priority,
    CASE
        WHEN deadline_type = 'on' AND scheduled_date IS NULL THEN deadline_date
        ELSE scheduled_date
    END AS scheduled_date,
    CASE
        WHEN deadline_type = 'on' AND scheduled_date IS NULL THEN deadline_time
        ELSE NULL
    END AS scheduled_time,
    CASE
        WHEN deadline_type = 'by'                                         THEN deadline_date
        WHEN deadline_type = 'on' AND scheduled_date IS NOT NULL
            AND scheduled_date != deadline_date                           THEN deadline_date
        ELSE NULL
    END AS deadline_date,
    CASE
        WHEN deadline_type = 'by'                                         THEN deadline_time
        WHEN deadline_type = 'on' AND scheduled_date IS NOT NULL
            AND scheduled_date != deadline_date                           THEN deadline_time
        ELSE NULL
    END AS deadline_time,
    recurrence, color_label_id, reminders, sound,
    created_at, updated_at, completed_at, etag
FROM tasks;

-- 3. Drop the old table. This also drops the FTS-sync triggers attached to
-- it (tasks_fts_ai / _ad / _au). Their definitions are recreated at the end
-- of this migration so search keeps working.
DROP TABLE tasks;

-- 4. Promote the new table to the canonical name.
ALTER TABLE tasks_new RENAME TO tasks;

-- 5. Recreate indexes (DROP TABLE also drops indexes attached to it).
CREATE INDEX tasks_list_id_idx        ON tasks(list_id);
CREATE INDEX tasks_parent_id_idx      ON tasks(parent_id);
CREATE INDEX tasks_scheduled_date_idx ON tasks(scheduled_date);
CREATE INDEX tasks_deadline_date_idx  ON tasks(deadline_date);

-- 6. Recreate the FTS sync triggers. The bodies are copied from 0002 — they
-- don't reference any of the dropped columns, so they're stable across the
-- schema change.
CREATE TRIGGER tasks_fts_ai AFTER INSERT ON tasks BEGIN
    INSERT INTO tasks_fts (id, title, description, list_name, color_label)
    VALUES (
        NEW.id,
        NEW.title,
        COALESCE(NEW.description, ''),
        COALESCE((SELECT name FROM task_lists    WHERE id = NEW.list_id),        ''),
        COALESCE((SELECT name FROM color_labels  WHERE id = NEW.color_label_id), '')
    );
END;

CREATE TRIGGER tasks_fts_ad AFTER DELETE ON tasks BEGIN
    DELETE FROM tasks_fts WHERE id = OLD.id;
END;

CREATE TRIGGER tasks_fts_au AFTER UPDATE ON tasks BEGIN
    DELETE FROM tasks_fts WHERE id = OLD.id;
    INSERT INTO tasks_fts (id, title, description, list_name, color_label)
    VALUES (
        NEW.id,
        NEW.title,
        COALESCE(NEW.description, ''),
        COALESCE((SELECT name FROM task_lists    WHERE id = NEW.list_id),        ''),
        COALESCE((SELECT name FROM color_labels  WHERE id = NEW.color_label_id), '')
    );
END;

-- 7. Recreate the rename triggers we dropped at step 0. Bodies match 0002.
CREATE TRIGGER task_lists_fts_rename AFTER UPDATE OF name ON task_lists BEGIN
    UPDATE tasks_fts
       SET list_name = NEW.name
     WHERE id IN (SELECT id FROM tasks WHERE list_id = NEW.id);
END;

CREATE TRIGGER color_labels_fts_rename AFTER UPDATE OF name ON color_labels BEGIN
    UPDATE events_fts
       SET color_label = NEW.name
     WHERE id IN (SELECT id FROM events WHERE color_label_id = NEW.id);
    UPDATE tasks_fts
       SET color_label = NEW.name
     WHERE id IN (SELECT id FROM tasks WHERE color_label_id = NEW.id);
END;
