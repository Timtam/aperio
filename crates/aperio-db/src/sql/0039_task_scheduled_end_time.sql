-- The end of a task's planned block.
--
-- `scheduled_time` says when the user means to start; this says when they mean
-- to stop, so a planned task can occupy a SPAN in the calendar the way an event
-- does instead of sitting at a single point. It is a time on the scheduled day:
-- the pair is (`scheduled_date`, `scheduled_time`) → (`scheduled_date`,
-- `scheduled_end_time`), and an end at or before the start is not a block, so
-- the writer drops it rather than storing a negative one.
--
-- It is an END rather than a duration because that is what the sources with a
-- real span store — CalDAV's DTSTART..DUE / DURATION pair and Vikunja's
-- `end_date` are both endpoints — and what a calendar grid draws. Todoist is
-- the exception, keeping an amount plus a unit, and its adapter derives that
-- from the two ends.
--
-- Nullable with no default: a task without a planned end is the normal case, so
-- every existing row backfills to NULL for free. Meaningless without
-- `scheduled_time`, which the writers enforce; no CHECK constraint is added
-- here because migration 0006's constraints live on the original table
-- definition and rebuilding the table for this would cost more than it buys.

ALTER TABLE tasks
    ADD COLUMN scheduled_end_time TEXT;  -- 'HH:MM:SS', NULL ⇒ no planned end
