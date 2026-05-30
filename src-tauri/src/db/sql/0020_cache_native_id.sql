-- Migration 0020: provider-native resource id on cached item rows
-- (CACHE-8 — true per-resource delta sync).
--
-- The cache keys items by the cal-core id, which several adapters encode
-- as a composite: CalDAV `{href}|{uid}`, EWS `{kind}:{item_id}|{change_key}`.
-- A delta sync's *deletion* only carries the provider-native resource id
-- (CalDAV href, EWS ItemId) — not the change-key/uid half — so it can't
-- reconstruct the full cal-core id to delete by. We store the native id
-- alongside each row so `apply_*_delta` can DELETE by it.
--
-- The native id is derived host-side by a single universal rule (strip a
-- leading one-char `X:` kind prefix, then take the substring before the
-- first `|`). For adapters whose id is already the native id (Google,
-- Graph, Vikunja) it equals the id. Existing rows backfill to '' and get
-- the real value on their next refresh write; the column is additive.

ALTER TABLE cache_events   ADD COLUMN native_id TEXT NOT NULL DEFAULT '';
ALTER TABLE cache_tasks    ADD COLUMN native_id TEXT NOT NULL DEFAULT '';
ALTER TABLE cache_contacts ADD COLUMN native_id TEXT NOT NULL DEFAULT '';

CREATE INDEX cache_events_native_idx
    ON cache_events(account_id, calendar_id, native_id);
CREATE INDEX cache_tasks_native_idx
    ON cache_tasks(account_id, list_id, native_id);
CREATE INDEX cache_contacts_native_idx
    ON cache_contacts(account_id, list_id, native_id);
