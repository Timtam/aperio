-- Migration 0012: idempotency table for the event-log applier
-- (DESIGN.md §19, Phase Sc).
--
-- Every sync envelope carries a globally-unique `event_id`. When
-- the applier processes a log file (its own re-imports, another
-- device's logs, a snapshot's tail of unfolded events), it
-- records the ids it has integrated here. Re-applying the same
-- file — common during onboarding, on a backfill after the cache
-- TTL expires, or simply when the sync adapter re-fetches a log
-- it hadn't deleted yet — becomes a no-op.
--
-- We deliberately do NOT key on `(event_id, device_id)`: the
-- event id alone is globally unique by construction (ULID-prefixed
-- random suffix in `sync-core::event::mint_event_id`), and the
-- table stays cheap.
--
-- Two retention concerns we explicitly punt to a later phase:
--
--   1. Pruning rows older than the current snapshot's timestamp.
--      Once a snapshot is generated (Phase Sg), every event-id
--      whose timestamp pre-dates the snapshot can be GC'd from
--      this table without changing correctness — the events are
--      already folded in.
--   2. Indexing on `applied_at`. Not needed today; the only
--      query is `SELECT 1 ... WHERE event_id = ?` which the PK
--      already covers.

CREATE TABLE sync_applied_events (
    event_id   TEXT NOT NULL PRIMARY KEY,
    applied_at TEXT NOT NULL
);
