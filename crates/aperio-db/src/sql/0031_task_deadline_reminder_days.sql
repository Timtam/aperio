-- Per-task deadline-countdown override (an Aperio-only field).
--
-- `deadline_reminder_days` overrides the global `tasks.deadlineCountdownDays`
-- setting for a single task: "remind me this many days before THIS task's
-- deadline" in the day-start review. It has no native home on any external
-- provider, so — like `effort` — on a LOCAL list it lives as an ordinary
-- column here, and on an external list it rides the AperioExtras bag
-- (extras.rs), carried only when set.
--
-- Nullable with no default: NULL means "use the global default", which is the
-- overwhelmingly common case, so existing rows backfill to NULL for free.

ALTER TABLE tasks
    ADD COLUMN deadline_reminder_days INTEGER;  -- NULL ⇒ use the global default
