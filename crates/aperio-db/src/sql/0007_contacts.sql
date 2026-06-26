-- Migration 0007: contacts foundation (DESIGN.md §10).
--
-- Adds two tables:
--
--   * `contact_lists` — the address-book container, one row per book
--     per account. Symmetric with `calendars` and `task_lists`.
--   * `contacts` — the contact rows themselves, with a single
--     `list_id` foreign key. Cross-list moves are a separate write
--     (delete + insert) handled at the command layer.
--
-- We don't add an FTS5 mirror here yet — the attendees-picker
-- autocomplete (§10.4) will need one, but it can land alongside the
-- picker itself in a later migration. Skipping FTS now keeps this
-- migration small and avoids the trigger-recreation dance migration
-- 0006 had to do for tasks.
--
-- No seed: a fresh install starts with no local address book. The
-- user creates one on demand (Sidebar "new address book" /
-- create_contact_list) and contacts are added to a list of their
-- choosing. Earlier revisions seeded a hard-coded
-- 'local-default-contacts' list here; existing installs keep theirs
-- (this migration already ran for them — user_version is past 7).

CREATE TABLE contact_lists (
    id              TEXT NOT NULL PRIMARY KEY,
    -- account_id is NOT NULL from the start (calendars / task_lists
    -- carry it as nullable for backwards compatibility; contacts are
    -- new, so we tighten the schema up front).
    account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    source          TEXT NOT NULL,            -- "local" | adapter source id
    name            TEXT NOT NULL,
    color_hex       TEXT,
    color_source    TEXT,                     -- "native" | "custom"
    read_only       INTEGER NOT NULL DEFAULT 0 CHECK (read_only IN (0, 1)),
    etag            TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX contact_lists_account_id_idx ON contact_lists(account_id);

CREATE TABLE contacts (
    id              TEXT NOT NULL PRIMARY KEY,
    list_id         TEXT NOT NULL REFERENCES contact_lists(id) ON DELETE CASCADE,
    display_name    TEXT NOT NULL,
    given_name      TEXT,
    family_name     TEXT,
    organization    TEXT,
    -- Emails and phone numbers are multi-valued. We store them as a
    -- JSON array — same trade-off the reminders column on tasks
    -- makes. Indexing by individual address is not a query path
    -- the app needs (lookups go through the FTS mirror once
    -- migration 0008 adds it).
    emails          TEXT NOT NULL DEFAULT '[]',
    phone_numbers   TEXT NOT NULL DEFAULT '[]',
    birthday        TEXT,                     -- ISO 8601 date (YYYY-MM-DD)
    notes           TEXT,
    etag            TEXT,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL
);

CREATE INDEX contacts_list_id_idx     ON contacts(list_id);
CREATE INDEX contacts_birthday_idx    ON contacts(birthday);
-- display_name is the primary sort key in every contacts UI surface
-- (alphabetised lists, picker results). An index here keeps the
-- listing query stable even when a user accumulates thousands of
-- contacts across address books.
CREATE INDEX contacts_display_name_idx ON contacts(display_name);
