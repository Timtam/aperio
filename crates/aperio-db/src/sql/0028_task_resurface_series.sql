-- On-demand / backlog recurrence: resurface date + series id (DESIGN §9.12).
--
-- `resurface_date` gates a backlog task's visibility: it surfaces in the
-- active backlog only on/after this day (until then it lives in the
-- "Zukünftig" group). `NULL` ⇒ visible now, i.e. the existing behavior.
--
-- `series_id` ties a recurring task's instances together so two Aperio
-- clients sharing a list don't each spawn the next instance — the spawner
-- only creates one when no open instance of the series exists.
--
-- Both ride the existing `tasks.*` event log as ordinary task fields. The
-- recurrence's new `anchor` / `placement` / `fixed_dates` axes live inside
-- the existing `recurrence` JSON column (serde), so they need no column.

ALTER TABLE tasks
    ADD COLUMN resurface_date TEXT;

ALTER TABLE tasks
    ADD COLUMN series_id TEXT;
