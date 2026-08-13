-- Labelled contact channels, plus the fields every provider has and Aperio
-- did not.
--
-- `emails` and `phone_numbers` are NOT rewritten here. They are JSON arrays,
-- and their element shape changed from a bare string to `{value, label}` —
-- which `ContactValue`'s deserialiser reads either way, on purpose. Rewriting
-- every row would give the same result as reading the old shape on demand, at
-- the cost of a migration that has to be right the first time on data the user
-- cannot get back. Rows adopt the new shape as they are next written.
--
-- `urls` gets a DEFAULT so existing rows read as an empty list rather than
-- NULL — the same treatment `addresses` had in 0011, and the reason its reader
-- can be a plain `req_text` instead of a nullable branch.

ALTER TABLE contacts
    ADD COLUMN urls TEXT NOT NULL DEFAULT '[]';  -- JSON [{value,label}]

ALTER TABLE contacts
    ADD COLUMN anniversary TEXT;  -- 'YYYY-MM-DD', NULL ⇒ none recorded

ALTER TABLE contacts
    ADD COLUMN job_title TEXT;

ALTER TABLE contacts
    ADD COLUMN department TEXT;

-- ── The search mirror indexes values, not the JSON around them ───────────
--
-- `contacts_fts.emails` is fed the raw `contacts.emails` text by the triggers
-- from 0008. With the old bare-string shape that was harmless: the only words
-- in `["max@example.com"]` were the address. The object shape puts the literal
-- words `value` and `label` in every row, and `search_contacts` appends `*` to
-- every token — so a search for "v" or "l" would match the entire address
-- book, burying the real hits under the 50-row cap.
--
-- The triggers now project the values out with `json_each`, branching on the
-- element's own `type`: an object yields its `value` member, a bare string
-- yields itself. The branch is not decoration — `json_extract` RAISES
-- "malformed JSON" on a plain string like `alt@example.com` rather than
-- returning NULL, so a COALESCE around it would make every write to a
-- pre-0040 row fail outright. Both shapes coexist by design, and both have to
-- index.
DROP TRIGGER contacts_fts_ai;
DROP TRIGGER contacts_fts_au;

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
        COALESCE(
            (SELECT group_concat(CASE je.type WHEN 'object' THEN json_extract(je.value, '$.value') ELSE je.value END, ' ')
               FROM json_each(COALESCE(NEW.emails, '[]')) je),
            ''
        ),
        COALESCE((SELECT name FROM contact_lists WHERE id = NEW.list_id), '')
    );
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
        COALESCE(
            (SELECT group_concat(CASE je.type WHEN 'object' THEN json_extract(je.value, '$.value') ELSE je.value END, ' ')
               FROM json_each(COALESCE(NEW.emails, '[]')) je),
            ''
        ),
        COALESCE((SELECT name FROM contact_lists WHERE id = NEW.list_id), '')
    );
END;

-- Re-index every existing row through the new projection: a contact written
-- by a build that already stored the object shape is sitting in the mirror
-- with `value`/`label` indexed right now.
DELETE FROM contacts_fts;
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
    COALESCE(
        (SELECT group_concat(CASE je.type WHEN 'object' THEN json_extract(je.value, '$.value') ELSE je.value END, ' ')
           FROM json_each(COALESCE(c.emails, '[]')) je),
        ''
    ),
    COALESCE((SELECT name FROM contact_lists WHERE id = c.list_id), '')
FROM contacts c;

-- ── Re-warm the external contact snapshots ───────────────────────────────
--
-- Same reasoning as 0021 did for events, and the stakes are higher here.
-- Cached contact rows written before this migration were mapped by an adapter
-- that never read `jobTitle`, `department`, `businessHomePage`, the postal
-- addresses or the anniversary — they decode as "the user has none of these",
-- because the new fields are `#[serde(default)]`. A delta sync does not
-- re-send an unchanged contact, so those blanks would persist.
--
-- That alone would only be a display gap. What makes it destructive is the
-- other half of this change: writes now send an absent field as an explicit
-- clear, so the first save of an untouched Outlook or Exchange contact would
-- delete the job title, department and website it really has on the server —
-- silently, because the editor showed them blank.
--
-- Dropping the sync state as well as the rows matters twice over: it forces a
-- full re-fetch, and on Microsoft Graph the stored deltaLink encodes the OLD
-- `$select`, so following it would keep returning payloads without the new
-- properties no matter how many rounds ran. The cache is disposable
-- (stale-while-revalidate); the cost is one refresh.
DELETE FROM cache_contacts;
DELETE FROM cache_sync_state WHERE scope = 'contacts';
