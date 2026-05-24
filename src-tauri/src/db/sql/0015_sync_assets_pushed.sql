-- Migration 0015: track which sound assets have been uploaded
-- to the sync store (DESIGN.md §19.10 / §19.11 step 7).
--
-- Sound files (custom notification sounds, referenced by sha256
-- in SoundConfig::Custom) live in two places: locally under
-- `<data_dir>/assets/sounds/<hash>.<ext>` and on the sync store
-- under `<remote>/assets/sounds/<hash>.<ext>`. The sync round
-- needs to push local-only files + fetch remote-only files;
-- this table holds the "we've already pushed this hash"
-- bookkeeping so a slow link doesn't re-upload every file on
-- every round.
--
-- ## Why a table rather than a user_prefs key per hash
--
-- Per-hash prefs would work but make the user_prefs whitelist
-- noisy and would inflate that table's row count for users with
-- many custom sounds. A dedicated table keeps the
-- responsibility separated.
--
-- ## Schema
--
-- - `hash` is the file's sha256 (lowercase hex). Primary key
--   because the hash is the entire identity of a sound asset.
-- - `extension` is the bare suffix (`mp3`, `ogg`, …) without
--   the dot. The adapter trait carries the extension separately
--   so we store it here to reuse on a future fetch / orphan
--   check.
-- - `pushed_at` is RFC3339 for human inspection during
--   debugging. Not used by the sync logic itself.
--
-- ## Reset semantics
--
-- A user that wants to force re-upload can `DELETE FROM
-- sync_assets_pushed`. The next round walks the local sounds
-- dir again and re-pushes every file. Cheap recovery.

CREATE TABLE sync_assets_pushed (
    hash        TEXT NOT NULL PRIMARY KEY,
    extension   TEXT NOT NULL,
    pushed_at   TEXT NOT NULL
);
