-- Phase 1 initial schema: calendars, events, task lists, tasks, color labels,
-- settings. Reminders and attendees are stored as JSON columns on the parent
-- row — they are small, never queried independently in Phase 1, and the JSON
-- shape mirrors the cal-core types one-to-one. If sub-queries on these
-- become hot in a later phase, they can be normalised by a follow-up migration.
--
-- Phase 6 (external adapters), Phase 7 (sync / event log), Phase 8 (plugins),
-- and Phase 4 (FTS5) each add their own tables in dedicated migrations.

CREATE TABLE calendars (
    id              TEXT NOT NULL PRIMARY KEY,
    source          TEXT NOT NULL,          -- AdapterSource (e.g. "local")
    name            TEXT NOT NULL,
    color_hex       TEXT,
    color_source    TEXT,                   -- "native" | "custom"
    read_only       INTEGER NOT NULL DEFAULT 0,
    default_sound   TEXT,                   -- JSON: SoundConfig
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE TABLE events (
    id              TEXT NOT NULL PRIMARY KEY,
    calendar_id     TEXT NOT NULL REFERENCES calendars(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    description     TEXT,
    location        TEXT,
    start_utc       TEXT NOT NULL,          -- RFC3339
    end_utc         TEXT NOT NULL,
    all_day         INTEGER NOT NULL DEFAULT 0,
    rrule           TEXT,                   -- RFC 5545 RRULE string
    rrule_exceptions TEXT,                  -- JSON array of RFC3339 timestamps
    color_label_id  TEXT REFERENCES color_labels(id) ON DELETE SET NULL,
    reminders       TEXT NOT NULL DEFAULT '[]',  -- JSON array of Reminder
    sound           TEXT,                   -- JSON: SoundConfig (override)
    attendees       TEXT NOT NULL DEFAULT '[]',  -- JSON array of email strings
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    etag            TEXT
);

CREATE INDEX events_calendar_id_idx ON events(calendar_id);
CREATE INDEX events_start_utc_idx   ON events(start_utc);

CREATE TABLE task_lists (
    id                      TEXT NOT NULL PRIMARY KEY,
    source                  TEXT NOT NULL,
    name                    TEXT NOT NULL,
    color_hex               TEXT,
    color_source            TEXT,
    default_sound           TEXT,
    embedded_in_calendar    TEXT REFERENCES calendars(id) ON DELETE CASCADE,
    read_only               INTEGER NOT NULL DEFAULT 0,
    created_at              TEXT NOT NULL,
    updated_at              TEXT NOT NULL
);

CREATE TABLE tasks (
    id              TEXT NOT NULL PRIMARY KEY,
    list_id         TEXT NOT NULL REFERENCES task_lists(id) ON DELETE CASCADE,
    parent_id       TEXT REFERENCES tasks(id) ON DELETE CASCADE,
    title           TEXT NOT NULL,
    description     TEXT,
    status          TEXT NOT NULL,          -- "open" | "in_progress" | "completed" | "cancelled"
    priority        TEXT NOT NULL,          -- "low" | "medium" | "high"
    scheduled_date  TEXT,                   -- ISO 8601 date
    deadline_type   TEXT,                   -- "on" | "by"
    deadline_date   TEXT,
    deadline_time   TEXT,                   -- HH:MM:SS, only when deadline_type='on'
    recurrence      TEXT,                   -- JSON: TaskRecurrence
    color_label_id  TEXT REFERENCES color_labels(id) ON DELETE SET NULL,
    reminders       TEXT NOT NULL DEFAULT '[]',
    sound           TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    completed_at    TEXT,
    etag            TEXT
);

CREATE INDEX tasks_list_id_idx       ON tasks(list_id);
CREATE INDEX tasks_parent_id_idx     ON tasks(parent_id);
CREATE INDEX tasks_scheduled_date_idx ON tasks(scheduled_date);
CREATE INDEX tasks_deadline_date_idx ON tasks(deadline_date);

CREATE TABLE color_labels (
    id      TEXT NOT NULL PRIMARY KEY,
    name    TEXT NOT NULL,
    hex     TEXT NOT NULL
);

CREATE TABLE settings (
    key     TEXT NOT NULL PRIMARY KEY,
    value   TEXT NOT NULL              -- JSON value, stored as-is
);
