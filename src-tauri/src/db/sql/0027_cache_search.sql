-- Extend full-text search to the EXTERNAL snapshot cache (§13.1).
--
-- The spec promises search over "alle lokal gecachten Termine und
-- Aufgaben über alle Konten und Container hinweg" — but only the LOCAL
-- events/tasks tables were ever indexed (migration 0002). Items living
-- in cache_events / cache_tasks (iCloud, Google, EWS, Vikunja, Todoist…)
-- were unfindable.
--
-- Mirrors the 0002 design: regular (content-bearing) FTS5 tables keyed
-- by UNINDEXED id columns, kept in lockstep by triggers on the cache
-- tables. Text fields are pulled out of the JSON payload via
-- json_extract at write time. The insert trigger deletes any stale row
-- for the same key first, so it stays correct regardless of which write
-- shape (plain INSERT after a container wipe, or upsert via
-- ON CONFLICT DO UPDATE) touched the base table. Range / type / status
-- filters don't need FTS columns — the search query joins back to the
-- base table and filters on its columns / payload.

CREATE VIRTUAL TABLE cache_events_fts USING fts5(
    account_id UNINDEXED,
    calendar_id UNINDEXED,
    id UNINDEXED,
    title,
    description,
    location,
    attendees,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE cache_tasks_fts USING fts5(
    account_id UNINDEXED,
    list_id UNINDEXED,
    id UNINDEXED,
    title,
    description,
    tokenize = 'unicode61 remove_diacritics 2'
);

-- ── Initial fill from the existing cache ────────────────────────────────

INSERT INTO cache_events_fts (account_id, calendar_id, id, title, description, location, attendees)
SELECT
    account_id,
    calendar_id,
    id,
    COALESCE(json_extract(payload, '$.title'), ''),
    COALESCE(json_extract(payload, '$.description'), ''),
    COALESCE(json_extract(payload, '$.location'), ''),
    COALESCE(json_extract(payload, '$.attendees'), '')
FROM cache_events;

INSERT INTO cache_tasks_fts (account_id, list_id, id, title, description)
SELECT
    account_id,
    list_id,
    id,
    COALESCE(json_extract(payload, '$.title'), ''),
    COALESCE(json_extract(payload, '$.description'), '')
FROM cache_tasks;

-- ── Triggers: keep the mirrors aligned ──────────────────────────────────

CREATE TRIGGER cache_events_fts_ai AFTER INSERT ON cache_events BEGIN
    DELETE FROM cache_events_fts
     WHERE account_id = new.account_id
       AND calendar_id = new.calendar_id
       AND id = new.id;
    INSERT INTO cache_events_fts (account_id, calendar_id, id, title, description, location, attendees)
    VALUES (
        new.account_id,
        new.calendar_id,
        new.id,
        COALESCE(json_extract(new.payload, '$.title'), ''),
        COALESCE(json_extract(new.payload, '$.description'), ''),
        COALESCE(json_extract(new.payload, '$.location'), ''),
        COALESCE(json_extract(new.payload, '$.attendees'), '')
    );
END;

CREATE TRIGGER cache_events_fts_au AFTER UPDATE ON cache_events BEGIN
    DELETE FROM cache_events_fts
     WHERE account_id = old.account_id
       AND calendar_id = old.calendar_id
       AND id = old.id;
    INSERT INTO cache_events_fts (account_id, calendar_id, id, title, description, location, attendees)
    VALUES (
        new.account_id,
        new.calendar_id,
        new.id,
        COALESCE(json_extract(new.payload, '$.title'), ''),
        COALESCE(json_extract(new.payload, '$.description'), ''),
        COALESCE(json_extract(new.payload, '$.location'), ''),
        COALESCE(json_extract(new.payload, '$.attendees'), '')
    );
END;

CREATE TRIGGER cache_events_fts_ad AFTER DELETE ON cache_events BEGIN
    DELETE FROM cache_events_fts
     WHERE account_id = old.account_id
       AND calendar_id = old.calendar_id
       AND id = old.id;
END;

CREATE TRIGGER cache_tasks_fts_ai AFTER INSERT ON cache_tasks BEGIN
    DELETE FROM cache_tasks_fts
     WHERE account_id = new.account_id
       AND list_id = new.list_id
       AND id = new.id;
    INSERT INTO cache_tasks_fts (account_id, list_id, id, title, description)
    VALUES (
        new.account_id,
        new.list_id,
        new.id,
        COALESCE(json_extract(new.payload, '$.title'), ''),
        COALESCE(json_extract(new.payload, '$.description'), '')
    );
END;

CREATE TRIGGER cache_tasks_fts_au AFTER UPDATE ON cache_tasks BEGIN
    DELETE FROM cache_tasks_fts
     WHERE account_id = old.account_id
       AND list_id = old.list_id
       AND id = old.id;
    INSERT INTO cache_tasks_fts (account_id, list_id, id, title, description)
    VALUES (
        new.account_id,
        new.list_id,
        new.id,
        COALESCE(json_extract(new.payload, '$.title'), ''),
        COALESCE(json_extract(new.payload, '$.description'), '')
    );
END;

CREATE TRIGGER cache_tasks_fts_ad AFTER DELETE ON cache_tasks BEGIN
    DELETE FROM cache_tasks_fts
     WHERE account_id = old.account_id
       AND list_id = old.list_id
       AND id = old.id;
END;
