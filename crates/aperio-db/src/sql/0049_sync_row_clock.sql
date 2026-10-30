-- The time of the newest change each synced row has seen
-- (DESIGN-sync-snapshot-coverage.md, review round 1 of #130). An event older
-- than its row's clock no longer wins — a change written offline and uploaded
-- late, or a grown session file fetched again, used to overwrite a newer value
-- wherever a kind takes the whole row (settings, sections, day markers, day
-- logs, accounts) or merged against the event's own time (task lists, colour
-- labels). The writer advances it for this device's own changes, the applier
-- for every change it takes; a delete keeps it, so an older create arriving
-- later stays deleted. Microseconds since the epoch: compared as numbers,
-- never as text.
CREATE TABLE sync_row_clock (
    kind  TEXT    NOT NULL,
    id    TEXT    NOT NULL,
    at_us INTEGER NOT NULL,
    PRIMARY KEY (kind, id)
);
