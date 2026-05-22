-- Migration 0008: FTS5 mirror for contacts (DESIGN.md §10.4 attendees
-- picker). Same pattern that 0002 set up for events / tasks: a virtual
-- table `contacts_fts` carrying the searchable columns plus a
-- denormalised `list_name`, kept in sync via INSERT / DELETE / UPDATE
-- triggers on the `contacts` table and a rename trigger on
-- `contact_lists`.
--
-- Why FTS5 here and not just LIKE: the EventDialog autocomplete needs
-- substring matching across display_name, given_name, family_name,
-- organization, and emails — five columns, multi-list fan-out — with
-- result ranking for typeahead UX. LIKE works for small contact
-- counts but scales linearly and offers no relevance scoring; FTS5
-- gives us BM25 ordering and prefix matching for free, on the same
-- tokenizer Aperio already uses for events / tasks (so umlauts /
-- accents fold consistently across all three).
--
-- Tokenizer: `unicode61 remove_diacritics 2` — same as events_fts and
-- tasks_fts. Diacritic-stripping is the right call for personal
-- contacts where "Müller" should match "Muller" without the user
-- having to think about it.

CREATE VIRTUAL TABLE contacts_fts USING fts5(
    id UNINDEXED,
    display_name,
    given_name,
    family_name,
    organization,
    emails,
    list_name,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- Backfill from the existing `contacts` table. The seed list from
-- migration 0007 has no contacts yet on the production path, but a
-- dev DB that already manually inserted rows (or a future restore /
-- import) wouldn't otherwise be searchable until something edited
-- each row.
INSERT INTO contacts_fts (
    id, display_name, given_name, family_name,
    organization, emails, list_name
)
SELECT
    c.id,
    c.display_name,
    COALESCE(c.given_name, ''),
    COALESCE(c.family_name, ''),
    COALESCE(c.organization, ''),
    COALESCE(c.emails, '[]'),
    COALESCE((SELECT name FROM contact_lists WHERE id = c.list_id), '')
FROM contacts c;

-- ── Triggers: contacts.* keep contacts_fts in sync ───────────────────────

CREATE TRIGGER contacts_fts_ai AFTER INSERT ON contacts BEGIN
    INSERT INTO contacts_fts (
        id, display_name, given_name, family_name,
        organization, emails, list_name
    )
    VALUES (
        NEW.id,
        NEW.display_name,
        COALESCE(NEW.given_name, ''),
        COALESCE(NEW.family_name, ''),
        COALESCE(NEW.organization, ''),
        COALESCE(NEW.emails, '[]'),
        COALESCE((SELECT name FROM contact_lists WHERE id = NEW.list_id), '')
    );
END;

CREATE TRIGGER contacts_fts_ad AFTER DELETE ON contacts BEGIN
    DELETE FROM contacts_fts WHERE id = OLD.id;
END;

CREATE TRIGGER contacts_fts_au AFTER UPDATE ON contacts BEGIN
    DELETE FROM contacts_fts WHERE id = OLD.id;
    INSERT INTO contacts_fts (
        id, display_name, given_name, family_name,
        organization, emails, list_name
    )
    VALUES (
        NEW.id,
        NEW.display_name,
        COALESCE(NEW.given_name, ''),
        COALESCE(NEW.family_name, ''),
        COALESCE(NEW.organization, ''),
        COALESCE(NEW.emails, '[]'),
        COALESCE((SELECT name FROM contact_lists WHERE id = NEW.list_id), '')
    );
END;

-- Keep the denormalised `list_name` fresh when a contact list is
-- renamed. Mirrors the calendars_fts_rename / task_lists_fts_rename
-- triggers from 0002.
CREATE TRIGGER contact_lists_fts_rename AFTER UPDATE OF name ON contact_lists BEGIN
    UPDATE contacts_fts
       SET list_name = NEW.name
     WHERE id IN (SELECT id FROM contacts WHERE list_id = NEW.id);
END;
