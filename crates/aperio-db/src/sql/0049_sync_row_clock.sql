-- Each synced row's clock (DESIGN-sync-snapshot-coverage.md, review round 1
-- of #130). An event older than its row's clock no longer wins — a change
-- written offline and uploaded late, or a grown session file fetched again,
-- used to overwrite a newer value wherever a kind takes the whole row
-- (settings, sections, day markers, day logs, accounts) or merged against the
-- event's own time (task lists, colour labels). The writer advances it for
-- this device's own changes, the applier for every change it takes.
--
-- `changed_us`: the newest change of any kind. `written_us`: the newest that
-- wrote the whole row (a create, a whole-row update, a delete), NULL while
-- only field merges touched it. `deleted`: that newest whole-row write was a
-- delete. A delete loses only to a newer whole-row write; a field merge onto
-- a row deleted here stays out, but not onto one its list or calendar took
-- along (no delete of its own). An older create arriving later stays
-- deleted. `seeded`: only seeded, from a change this device had applied, on
-- a row of unknown time; the first change taken on the row clears it, and a
-- seed never marks a known clock (review round 5). Microseconds since the
-- epoch: compared as numbers, never as text.
CREATE TABLE sync_row_clock (
    kind       TEXT    NOT NULL,
    id         TEXT    NOT NULL,
    changed_us INTEGER NOT NULL,
    written_us INTEGER,
    deleted    INTEGER NOT NULL DEFAULT 0,
    seeded     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (kind, id)
);
