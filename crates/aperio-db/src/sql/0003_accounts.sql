-- Phase 6a: account model.
--
-- Every external adapter Aperio talks to lives behind an "account" — a
-- saved instance of an adapter together with the user-visible name and
-- whatever non-secret configuration it needs (server URL, calendar
-- root path, OAuth client config, ...). Secrets stay out of the DB and
-- live in the platform keychain instead (DESIGN.md §6.6).
--
-- We also pre-create one well-known row for the local adapter so the
-- foreign keys from `calendars.account_id` and `task_lists.account_id`
-- can be made NOT NULL eventually. For now the column is nullable to
-- keep the migration backwards-compatible with rows written before this
-- migration ran.

CREATE TABLE accounts (
    id              TEXT NOT NULL PRIMARY KEY,
    adapter_kind    TEXT NOT NULL,            -- "local" | "caldav" | "google" | ...
    display_name    TEXT NOT NULL,
    config_json     TEXT NOT NULL DEFAULT '{}',
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX accounts_adapter_kind_idx ON accounts(adapter_kind);

-- Seed the implicit local account. Its id matches the AdapterSource
-- the local adapter has hard-coded (`adapter-local::SOURCE_ID`),
-- so existing rows that already carry `source = 'local'` can be
-- attached without a value rewrite.
INSERT INTO accounts (id, adapter_kind, display_name, config_json, created_at, updated_at)
VALUES (
    'local',
    'local',
    'Local',
    '{}',
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
);

-- Hang each container off an account. The columns are nullable for the
-- duration of the migration so we can backfill existing rows in the
-- same transaction without a strict-NOT-NULL fight; a follow-up
-- migration in a later phase will tighten this once every code path
-- guarantees a value.
ALTER TABLE calendars  ADD COLUMN account_id TEXT REFERENCES accounts(id) ON DELETE CASCADE;
ALTER TABLE task_lists ADD COLUMN account_id TEXT REFERENCES accounts(id) ON DELETE CASCADE;

UPDATE calendars  SET account_id = 'local' WHERE account_id IS NULL;
UPDATE task_lists SET account_id = 'local' WHERE account_id IS NULL;

CREATE INDEX calendars_account_id_idx  ON calendars(account_id);
CREATE INDEX task_lists_account_id_idx ON task_lists(account_id);
