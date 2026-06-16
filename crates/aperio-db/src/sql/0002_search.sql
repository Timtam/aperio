-- Phase 4 search: FTS5 indexes over events and tasks plus the triggers
-- that keep them aligned with the canonical rows.
--
-- The indexes are *contentless* (no `content=` option) and carry the
-- TEXT primary key as an UNINDEXED column. We can't use the typical
-- external-content pattern because `events.id` / `tasks.id` are UUID
-- strings, not integer rowids — FTS5 external content requires an
-- integer key. Maintaining the indexes by trigger is straightforward
-- and keeps every write path consistent.
--
-- Denormalised columns (`calendar_name`, `list_name`, `color_label`)
-- are filled in via subquery at insert time; rename triggers on the
-- referenced tables propagate later updates back into the index.

CREATE VIRTUAL TABLE events_fts USING fts5(
    id UNINDEXED,
    title,
    description,
    location,
    attendees,
    calendar_name,
    color_label,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE tasks_fts USING fts5(
    id UNINDEXED,
    title,
    description,
    list_name,
    color_label,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- ── Initial fill ─────────────────────────────────────────────────────────

INSERT INTO events_fts (id, title, description, location, attendees, calendar_name, color_label)
SELECT
    e.id,
    e.title,
    COALESCE(e.description, ''),
    COALESCE(e.location, ''),
    COALESCE(e.attendees, '[]'),
    COALESCE((SELECT name FROM calendars     WHERE id = e.calendar_id),   ''),
    COALESCE((SELECT name FROM color_labels  WHERE id = e.color_label_id), '')
FROM events e;

INSERT INTO tasks_fts (id, title, description, list_name, color_label)
SELECT
    t.id,
    t.title,
    COALESCE(t.description, ''),
    COALESCE((SELECT name FROM task_lists    WHERE id = t.list_id),        ''),
    COALESCE((SELECT name FROM color_labels  WHERE id = t.color_label_id), '')
FROM tasks t;

-- ── Triggers: events.* keep events_fts in sync ───────────────────────────

CREATE TRIGGER events_fts_ai AFTER INSERT ON events BEGIN
    INSERT INTO events_fts (id, title, description, location, attendees, calendar_name, color_label)
    VALUES (
        NEW.id,
        NEW.title,
        COALESCE(NEW.description, ''),
        COALESCE(NEW.location, ''),
        COALESCE(NEW.attendees, '[]'),
        COALESCE((SELECT name FROM calendars     WHERE id = NEW.calendar_id),   ''),
        COALESCE((SELECT name FROM color_labels  WHERE id = NEW.color_label_id), '')
    );
END;

CREATE TRIGGER events_fts_ad AFTER DELETE ON events BEGIN
    DELETE FROM events_fts WHERE id = OLD.id;
END;

CREATE TRIGGER events_fts_au AFTER UPDATE ON events BEGIN
    DELETE FROM events_fts WHERE id = OLD.id;
    INSERT INTO events_fts (id, title, description, location, attendees, calendar_name, color_label)
    VALUES (
        NEW.id,
        NEW.title,
        COALESCE(NEW.description, ''),
        COALESCE(NEW.location, ''),
        COALESCE(NEW.attendees, '[]'),
        COALESCE((SELECT name FROM calendars     WHERE id = NEW.calendar_id),   ''),
        COALESCE((SELECT name FROM color_labels  WHERE id = NEW.color_label_id), '')
    );
END;

-- ── Triggers: tasks.* keep tasks_fts in sync ─────────────────────────────

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

-- ── Rename triggers: keep denormalised name columns fresh ────────────────

CREATE TRIGGER calendars_fts_rename AFTER UPDATE OF name ON calendars BEGIN
    UPDATE events_fts
       SET calendar_name = NEW.name
     WHERE id IN (SELECT id FROM events WHERE calendar_id = NEW.id);
END;

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

-- When a color label is deleted, ON DELETE SET NULL on the referencing
-- columns fires the UPDATE triggers on events/tasks, which already
-- rewrites the matching FTS rows. No extra delete trigger needed.
