-- Day markers: a small, user-defined vocabulary of things worth noting about a
-- DAY, and one record per day saying which of them applied.
--
-- Why this is not tasks, which is where the idea started: a task has ONE
-- status, and this needs one per (marker, day) — "meditated Monday, not
-- Tuesday, again Wednesday". Modelling it as recurring tasks would put a year
-- of spawned instances in the planner the user explicitly does not want them
-- in, push thousands of rows at Vikunja/Todoist for a list they cannot
-- interpret, and leave every task surface in the app carrying an "unless it is
-- a habit list" clause. This is an annotation on the date itself, so it gets
-- its own two small tables.

-- The vocabulary. Free-form on purpose: the name carries whatever the user
-- wants it to (a word, a sentence, an emoji), and `symbol` is the short form
-- the compact per-day summaries render when there is no room for the name.
CREATE TABLE day_markers (
    id          TEXT NOT NULL PRIMARY KEY,
    name        TEXT NOT NULL,
    -- Short stand-in for the dense views — typically one emoji. NULL ⇒ the
    -- summaries fall back to the name's first characters.
    symbol      TEXT,
    -- Reuses the existing colour vocabulary rather than inventing a second
    -- one. ON DELETE SET NULL: losing a colour must not lose the marker.
    color_label TEXT REFERENCES color_labels(id) ON DELETE SET NULL,
    -- User-chosen order. The pickers and summaries read in this order so the
    -- list a user built reads back the way they built it.
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);

CREATE INDEX day_markers_position_idx ON day_markers(position);

-- One row per day. `markers` is a JSON array of `day_markers.id`.
--
-- A row rather than one row per (day, marker) because every read is
-- "what about this day" or "what about this month" — a month view wants 31
-- rows, not 31 × N. The write is a whole-row replace, which is the same
-- last-write-wins the rest of the synced store uses: two devices editing the
-- SAME day between two sync rounds keep the later edit, not the union. That is
-- a real (if narrow) loss, and it is chosen deliberately — a union would make
-- REMOVING a marker impossible to propagate, which is the worse failure.
--
-- No FK from the JSON to `day_markers`: SQLite cannot express one, and the
-- readers already drop ids they cannot resolve, which is also what makes a
-- deleted marker disappear from history without a migration.
CREATE TABLE day_log (
    -- Local calendar day, 'YYYY-MM-DD'. The LOCAL day is the point: the
    -- question is "how was Tuesday", not "what happened in this UTC window".
    day        TEXT NOT NULL PRIMARY KEY,
    markers    TEXT NOT NULL DEFAULT '[]',
    -- Room the design left open: a single scalar "how was today" that a later
    -- stage can fill without a second migration or a second table. NULL until
    -- then, and every reader today treats it as absent.
    rating     INTEGER,
    updated_at TEXT NOT NULL
);
