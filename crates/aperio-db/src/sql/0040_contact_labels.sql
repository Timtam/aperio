-- Labelled contact channels, plus the fields every provider has and Aperio
-- did not.
--
-- `emails` and `phone_numbers` are NOT touched here. They are JSON arrays, and
-- their element shape changed from a bare string to `{value, label}` — which
-- `ContactValue`'s deserialiser reads either way, on purpose. Rewriting every
-- row would give the same result as reading the old shape on demand, at the
-- cost of a migration that has to be right the first time on data the user
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
