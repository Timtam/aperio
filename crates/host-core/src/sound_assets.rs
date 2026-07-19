//! Sound-asset sync (DESIGN.md §19.10 / §19.11 step 7).
//!
//! Custom notification sounds (referenced by sha256 in
//! [`cal_core::reminder::SoundSource::Custom`]) are binary blobs
//! that live OUT-OF-BAND from the event log. They never travel
//! through the sync log itself — only their hash references do,
//! attached to events / tasks / reminders. The actual audio
//! bytes flow through the SyncAdapter's `push_sound_asset` /
//! `fetch_sound_asset` calls keyed by hash + extension.
//!
//! ## Two halves of the round
//!
//! ### Push (this device → remote)
//!
//! Walk the local `<data_dir>/assets/sounds/` directory. Each
//! file is named `<sha256>.<ext>` — the importer (whenever it
//! lands; see TODO in §19.11.7) is responsible for that
//! convention. For every (hash, ext) pair not yet recorded in
//! `sync_assets_pushed`, push to the adapter and record the
//! row. The adapter is content-addressed; a duplicate upload is
//! a cheap no-op on the remote side, but the local marker
//! avoids re-reading the bytes from disk on every round.
//!
//! ### Fetch (remote → this device)
//!
//! Enumerate every sha256 referenced from the synced state
//! (events.sound, events.reminders[].sound, tasks.sound,
//! tasks.reminders[].sound, calendars.default_sound,
//! task_lists.default_sound). For each hash not present locally
//! under any extension, try a small ordered list of common
//! audio extensions (`mp3`, `ogg`, `wav`, `m4a`, `aac`, `flac`)
//! and accept the first hit. Save under `<hash>.<ext>`. Missing
//! sounds aren't an error — playback falls back to silence per
//! the §19.11.7 contract.
//!
//! ## Why an extension search vs a manifest file
//!
//! cal-core's `SoundSource::Custom` only carries the hash, not
//! the extension — but the SyncAdapter trait needs both. We could
//! push a `<remote>/assets/sounds/manifest.json` mapping hashes
//! to extensions, but the ordered probe is simpler, doesn't
//! introduce a parallel control file, and the wasted round-trips
//! per missing extension are negligible compared to one snapshot
//! pull. If profiling on a real dataset shows otherwise we can
//! add the manifest later without touching the data model.
//!
//! ## Best-effort throughout
//!
//! Every call site invokes this module's entry point inside an
//! `if let Err(err) = … { warn!(…) }` block. A failed push or
//! fetch is logged but doesn't sink the surrounding sync round
//! — the user data has converged correctly through the event
//! log; only the audio playback would be missing, and the next
//! round picks up where this one left off.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use chrono::Utc;
use rusqlite::params;
use serde::Serialize;
use sync_core::{SyncAdapter, SyncError, SyncResult};
use tracing::{debug, warn};

use crate::db::SharedConn;

/// Extensions we try (in order) when fetching a referenced
/// sound that isn't present locally. First hit wins. Covers the
/// formats the average user might import; exotic formats
/// (`.opus`, `.wma`) would need to be added when someone hits
/// them in the wild.
pub const FETCH_EXTENSION_CANDIDATES: &[&str] = &["mp3", "ogg", "wav", "m4a", "aac", "flac"];

/// Counts surfaced from one `sync_assets` invocation. The
/// orchestrator folds these into nothing yet — the sync-log
/// schema doesn't have an "asset" column — but a future polish
/// pass could add them to `SyncRoundReport`. For now they're
/// returned so the call sites can log them at info!.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct SoundAssetReport {
    /// Files pushed in this pass (newly uploaded; ignores
    /// files we'd already marked in `sync_assets_pushed`).
    pub pushed: usize,
    /// Hashes we fetched from the remote because they were
    /// referenced but missing locally.
    pub fetched: usize,
    /// Referenced hashes the remote didn't have under any of
    /// the candidate extensions. Logged at warn; the user sees
    /// silent reminders for these until someone re-imports.
    pub missing_on_remote: usize,
    /// Failures during push / fetch / disk IO. Counted so the
    /// caller can decide whether to surface a soft warning.
    pub failed: usize,
}

/// Sync the sound asset store between local + remote.
///
/// See module docs for the algorithm. The `sounds_dir` is the
/// canonical location for local sound files
/// (`<data_dir>/assets/sounds/`). Caller is responsible for
/// creating the directory if it doesn't exist yet — this
/// function tolerates a missing dir by treating it as "no local
/// sounds" (the fetch half can still work).
pub async fn sync_assets(
    db: &SharedConn,
    sounds_dir: &Path,
    adapter: &dyn SyncAdapter,
) -> SyncResult<SoundAssetReport> {
    let mut report = SoundAssetReport::default();

    // -- push half ----------------------------------------------------
    match list_local_sounds(sounds_dir) {
        Ok(local) => {
            for (hash, extension) in local {
                if hash_was_pushed(db, &hash) {
                    continue;
                }
                let path = sounds_dir.join(format!("{hash}.{extension}"));
                let bytes = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(err) => {
                        warn!(?path, ?err, "couldn't read local sound for push",);
                        report.failed += 1;
                        continue;
                    }
                };
                match adapter.push_sound_asset(&hash, &extension, &bytes).await {
                    Ok(()) => {
                        // The hash exists remotely now — lift any
                        // missing-on-remote back-off so peers' fetch halves
                        // (and ours, should the local file vanish) see it.
                        clear_missing(&hash);
                        if let Err(err) = mark_hash_pushed(db, &hash, &extension) {
                            warn!(
                                hash = %hash,
                                ?err,
                                "couldn't persist sync_assets_pushed row",
                            );
                            // Don't count as a failure — the
                            // push succeeded; we'll just re-push
                            // next round. Cheap.
                        }
                        report.pushed += 1;
                    }
                    Err(err) => {
                        warn!(
                            hash = %hash,
                            ?err,
                            "push_sound_asset failed",
                        );
                        report.failed += 1;
                    }
                }
            }
        }
        Err(err) => {
            // A missing dir was already filtered to Ok(empty);
            // anything else here is a real IO problem.
            warn!(?err, "couldn't enumerate local sounds dir");
            report.failed += 1;
        }
    }

    // -- fetch half ---------------------------------------------------
    let referenced = match referenced_hashes(db) {
        Ok(set) => set,
        Err(err) => {
            warn!(?err, "couldn't enumerate referenced sound hashes");
            return Ok(report);
        }
    };
    for hash in referenced {
        if local_hash_present(sounds_dir, &hash).is_some() {
            continue;
        }
        // Negative cache: a hash that probed missing recently is skipped —
        // without this, one dangling sound reference cost the FULL
        // extension-probe fan-out (6 sequential 404 GETs) on EVERY round,
        // often seconds of pure waste per round on a slow server.
        if missing_recently(&hash) {
            report.missing_on_remote += 1;
            continue;
        }
        // Probe each candidate extension; accept the first hit.
        let mut found = false;
        for ext in FETCH_EXTENSION_CANDIDATES {
            match adapter.fetch_sound_asset(&hash, ext).await {
                Ok(Some(bytes)) => {
                    if let Err(err) = write_local_sound(sounds_dir, &hash, ext, &bytes) {
                        warn!(
                            hash = %hash,
                            ?err,
                            "couldn't write fetched sound",
                        );
                        report.failed += 1;
                    } else {
                        report.fetched += 1;
                        // The remote already has this hash — we
                        // just fetched it from there. Record it
                        // in `sync_assets_pushed` so the next
                        // round's push half doesn't see the
                        // freshly-saved local file and try to
                        // re-upload identical bytes.
                        if let Err(err) = mark_hash_pushed(db, &hash, ext) {
                            warn!(
                                hash = %hash,
                                ?err,
                                "couldn't mark fetched sound as pushed",
                            );
                        }
                    }
                    found = true;
                    break;
                }
                Ok(None) => {
                    // Try the next extension.
                }
                Err(err) => {
                    debug!(
                        hash = %hash,
                        ext = ext,
                        ?err,
                        "fetch_sound_asset probe failed",
                    );
                }
            }
        }
        if !found {
            warn!(
                hash = %hash,
                "referenced sound not found on remote under any candidate extension",
            );
            report.missing_on_remote += 1;
            mark_missing(&hash);
        }
    }

    Ok(report)
}

/// In-memory negative cache for remote-missing sound hashes: hash →
/// when the last full probe came up empty. Process-wide and NOT
/// persisted, so a restart re-probes once; a peer uploading the sound
/// during the back-off appears after [`MISSING_RETRY_AFTER`] (or the
/// next restart), which is acceptable for notification sounds. A local
/// push of the hash clears its entry immediately.
fn missing_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Instant>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Instant>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Back-off before a remote-missing hash is probed again. One hour: long
/// enough to stop a permanently-dangling reference from costing the full
/// extension fan-out on every ~5-minute round, short enough that a sound
/// a PEER uploads shortly after we probed appears within the hour (there
/// is no cross-device invalidation signal).
const MISSING_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60 * 60);

/// Forget every remote-missing verdict. Call when the sync backend is
/// (re)configured — the verdicts were about a DIFFERENT remote (mirrors
/// the orchestrator clearing its per-session pushed-length map).
pub fn reset_missing_cache() {
    missing_cache()
        .lock()
        .expect("missing-sound cache poisoned")
        .clear();
}

fn missing_recently(hash: &str) -> bool {
    missing_cache()
        .lock()
        .expect("missing-sound cache poisoned")
        .get(hash)
        .is_some_and(|at| at.elapsed() < MISSING_RETRY_AFTER)
}

fn mark_missing(hash: &str) {
    missing_cache()
        .lock()
        .expect("missing-sound cache poisoned")
        .insert(hash.to_string(), Instant::now());
}

fn clear_missing(hash: &str) {
    missing_cache()
        .lock()
        .expect("missing-sound cache poisoned")
        .remove(hash);
}

/// Walk `sounds_dir` and return `(hash, extension)` for every
/// file named `<hash>.<ext>`. Files with no extension or that
/// don't parse as `<hex>.<ext>` are silently skipped.
///
/// A missing directory returns `Ok(Vec::new())` — the importer
/// hasn't run yet on this device.
pub fn list_local_sounds(sounds_dir: &Path) -> SyncResult<Vec<(String, String)>> {
    let read = match std::fs::read_dir(sounds_dir) {
        Ok(r) => r,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Vec::new());
        }
        Err(err) => return Err(SyncError::io(err.to_string())),
    };
    let mut out = Vec::new();
    for entry in read {
        let entry = match entry {
            Ok(e) => e,
            Err(err) => {
                debug!(?err, "skipping sounds dir entry");
                continue;
            }
        };
        let path = entry.path();
        let Some(stem) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string)
        else {
            continue;
        };
        let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
            continue;
        };
        // Basic hex shape check — keeps stray files (`.DS_Store`,
        // editor swap files, …) out of the push list without
        // needing a full sha256 parse.
        if !is_sha256_hex(&stem) {
            continue;
        }
        out.push((stem, ext.to_string()));
    }
    Ok(out)
}

/// Returns the extension under which a sound for `hash` exists
/// locally, or `None` if no matching file is found. We check
/// the common candidate extensions in the same order as the
/// fetch path; if the importer chose an exotic extension the
/// caller may fall back to re-fetch (idempotent), which isn't
/// ideal but doesn't cause data loss.
fn local_hash_present(sounds_dir: &Path, hash: &str) -> Option<String> {
    for ext in FETCH_EXTENSION_CANDIDATES {
        if sounds_dir.join(format!("{hash}.{ext}")).exists() {
            return Some((*ext).to_string());
        }
    }
    None
}

/// Persist the fetched bytes under `<sounds_dir>/<hash>.<ext>`.
/// Creates the directory if it doesn't exist yet.
fn write_local_sound(
    sounds_dir: &Path,
    hash: &str,
    extension: &str,
    bytes: &[u8],
) -> SyncResult<()> {
    if let Err(err) = std::fs::create_dir_all(sounds_dir) {
        return Err(SyncError::io(format!("create sounds dir: {err}")));
    }
    let path = sounds_dir.join(format!("{hash}.{extension}"));
    std::fs::write(&path, bytes)
        .map_err(|err| SyncError::io(format!("write sound {path:?}: {err}")))
}

/// Look up whether `hash` is already in `sync_assets_pushed`.
fn hash_was_pushed(db: &SharedConn, hash: &str) -> bool {
    let conn = db.lock().expect("db mutex poisoned");
    let mut stmt = match conn.prepare("SELECT 1 FROM sync_assets_pushed WHERE hash = ? LIMIT 1") {
        Ok(s) => s,
        Err(err) => {
            warn!(?err, "couldn't prepare sync_assets_pushed lookup");
            return false;
        }
    };
    stmt.exists(params![hash]).unwrap_or(false)
}

/// Record a successful push so the next round skips it.
fn mark_hash_pushed(db: &SharedConn, hash: &str, extension: &str) -> rusqlite::Result<()> {
    let conn = db.lock().expect("db mutex poisoned");
    conn.execute(
        "INSERT OR REPLACE INTO sync_assets_pushed
            (hash, extension, pushed_at)
         VALUES (?, ?, ?)",
        params![hash, extension, Utc::now().to_rfc3339()],
    )?;
    Ok(())
}

/// Run a UNION-style query against every JSON column that can
/// carry a `SoundSource::Custom` and return the set of
/// referenced sha256 hashes.
///
/// The SoundConfig shape in cal-core is
/// `{ source: { type, sha256?, … }, volume }`, so the JSON path
/// `$.source.sha256` is what we extract. `json_each` lets us
/// walk the reminders arrays per row without exploding them in
/// the application layer.
///
/// The final UNION branch covers the §14.4 sound *overrides* stored in
/// `user_prefs` (`sound.global`, `sound.calendar.{id}`,
/// `sound.tasklist.{id}`, `sound.item.{id}`) — those values are whole
/// `SoundConfig` JSON blobs, so the path is `$.source.sha256`. Without
/// this branch a custom sound referenced ONLY through a pref (e.g. the
/// global default) would never get pushed, and a second device could
/// never fetch it.
fn referenced_hashes(db: &SharedConn) -> rusqlite::Result<HashSet<String>> {
    // One CTE per source. SQLite's JSON1 is lenient — `json_each`
    // on NULL returns no rows, `json_extract` on missing paths
    // returns NULL, and our outer `IS NOT NULL` filter drops
    // those.
    let sql = "
        SELECT hash FROM (
            SELECT json_extract(sound, '$.source.sha256') AS hash
              FROM events
            UNION
            SELECT json_extract(sound, '$.source.sha256') AS hash
              FROM tasks
            UNION
            SELECT json_extract(default_sound, '$.source.sha256') AS hash
              FROM calendars
            UNION
            SELECT json_extract(default_sound, '$.source.sha256') AS hash
              FROM task_lists
            UNION
            SELECT json_extract(r.value, '$.sound.source.sha256') AS hash
              FROM events, json_each(events.reminders) AS r
            UNION
            SELECT json_extract(r.value, '$.sound.source.sha256') AS hash
              FROM tasks, json_each(tasks.reminders) AS r
            UNION
            SELECT json_extract(value, '$.source.sha256') AS hash
              FROM user_prefs WHERE key LIKE 'sound.%'
        )
        WHERE hash IS NOT NULL
    ";
    let conn = db.lock().expect("db mutex poisoned");
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut out = HashSet::new();
    for r in rows {
        out.insert(r?);
    }
    Ok(out)
}

/// Cheap sanity check before pushing — drops `.DS_Store`,
/// editor swap files, etc. from the dir walk. A real sha256 is
/// 64 lowercase hex characters; we don't normalise case here so
/// an importer that wrote uppercase would silently get skipped.
/// If that becomes a problem we can lowercase before the check.
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Path helper. The orchestrator + onboarding both want the
/// same place; this keeps the convention in one spot.
pub fn sounds_dir_under(data_dir: &Path) -> PathBuf {
    data_dir.join("assets").join("sounds")
}

/// Resolve the on-disk path of a custom sound by hash, probing the
/// candidate extensions in the same order as the fetch path. Returns
/// `None` when no file for `hash` exists locally — the reminder
/// scheduler (§14.4) treats that as "fall back to the system sound".
pub fn local_sound_path(sounds_dir: &Path, hash: &str) -> Option<PathBuf> {
    local_hash_present(sounds_dir, hash).map(|ext| sounds_dir.join(format!("{hash}.{ext}")))
}

// ── Import (§14.4 / §19.2.2) ─────────────────────────────────────────────────
//
// The content-addressed importer that gets a user's audio file onto disk under
// the `<sha256>.<ext>` convention the sync + resolution layers expect. Lives
// here (not the desktop command layer) so the desktop backend AND the mobile
// cal-ffi Host import through the same validation + hashing — one source of
// truth for the size cap + the accepted formats.

/// Max size of an imported sound (the §19.2.2 cap): large enough for any
/// realistic notification chime, small enough that syncing the blob stays cheap.
pub const MAX_SOUND_BYTES: u64 = 5 * 1024 * 1024;

/// Audio container extensions accepted on import — the same set the fetch path
/// probes (and the desktop player's decoders handle). Aliased to
/// [`FETCH_EXTENSION_CANDIDATES`] so the import allowlist can never drift from
/// what a second device will look for.
pub const ALLOWED_IMPORT_EXTENSIONS: &[&str] = FETCH_EXTENSION_CANDIDATES;

/// A custom sound on disk, identified by content hash + container extension.
/// Callers persist only `sha256` (in a `SoundConfig`); `ext` is informational
/// for the picker's list UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportedSound {
    pub sha256: String,
    pub ext: String,
}

/// Why an import was rejected. The desktop maps this to a `CommandError`, the
/// mobile cal-ffi Host to a `StoreError`.
#[derive(Debug, thiserror::Error)]
pub enum ImportSoundError {
    #[error("unsupported sound format; allowed: {0}")]
    UnsupportedFormat(String),
    #[error("sound file too large ({size} bytes); limit is {limit} bytes")]
    TooLarge { size: u64, limit: u64 },
    #[error("{0}")]
    Io(String),
}

/// Import an audio file at `src` into the content-addressed store under
/// `<sounds_dir>/<sha256>.<ext>`. Enforces the extension + size limits,
/// content-hashes the bytes, and writes (a no-op if identical bytes are already
/// stored under any extension). Returns the hash + extension so the caller can
/// write `SoundSource::Custom { sha256 }` into the relevant pref.
pub fn import_sound(sounds_dir: &Path, src: &Path) -> Result<ImportedSound, ImportSoundError> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| ALLOWED_IMPORT_EXTENSIONS.contains(&e.as_str()))
        .ok_or_else(|| ImportSoundError::UnsupportedFormat(ALLOWED_IMPORT_EXTENSIONS.join(", ")))?;

    // Size-gate via metadata before reading the whole file into memory.
    let meta = std::fs::metadata(src)
        .map_err(|e| ImportSoundError::Io(format!("cannot read sound: {e}")))?;
    if meta.len() > MAX_SOUND_BYTES {
        return Err(ImportSoundError::TooLarge {
            size: meta.len(),
            limit: MAX_SOUND_BYTES,
        });
    }

    let bytes =
        std::fs::read(src).map_err(|e| ImportSoundError::Io(format!("cannot read sound: {e}")))?;
    let sha256 = hex_digest(&bytes);

    std::fs::create_dir_all(sounds_dir)
        .map_err(|e| ImportSoundError::Io(format!("cannot create sounds dir: {e}")))?;
    // Content-addressed: if these exact bytes are already stored under any
    // extension, don't write a second copy.
    if local_sound_path(sounds_dir, &sha256).is_none() {
        let dest = sounds_dir.join(format!("{sha256}.{ext}"));
        std::fs::write(&dest, &bytes)
            .map_err(|e| ImportSoundError::Io(format!("cannot write sound: {e}")))?;
    }
    Ok(ImportedSound { sha256, ext })
}

/// Lowercase hex SHA-256 of `bytes`.
fn hex_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use chrono::Utc;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }

    fn insert_calendar(db: &SharedConn, id: &str, default_sound: Option<&str>) {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO calendars (
                id, source, name, color_hex, color_source, read_only,
                default_sound, created_at, updated_at
             ) VALUES (?, 'local', ?, NULL, NULL, 0, ?, ?, ?)",
            params![
                id,
                id,
                default_sound,
                Utc::now().to_rfc3339(),
                Utc::now().to_rfc3339()
            ],
        )
        .unwrap();
    }

    fn insert_event_with_reminders(
        db: &SharedConn,
        id: &str,
        calendar_id: &str,
        reminders_json: &str,
    ) {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO events (
                id, calendar_id, title, start_utc, end_utc, all_day,
                reminders, sound, attendees, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 0, ?, NULL, '[]', ?, ?)",
            params![
                id,
                calendar_id,
                id,
                Utc::now().to_rfc3339(),
                Utc::now().to_rfc3339(),
                reminders_json,
                Utc::now().to_rfc3339(),
                Utc::now().to_rfc3339(),
            ],
        )
        .unwrap();
    }

    #[test]
    fn is_sha256_hex_accepts_a_real_one() {
        // sha256 of empty bytes = e3b0c44298fc1c149afbf4c8996fb924…
        assert!(is_sha256_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
    }

    #[test]
    fn is_sha256_hex_rejects_short_or_non_hex() {
        assert!(!is_sha256_hex("nothex"));
        assert!(!is_sha256_hex(".DS_Store"));
        // 63 chars
        assert!(!is_sha256_hex(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85"
        ));
    }

    #[test]
    fn referenced_hashes_collects_from_calendar_default_sound() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let hash = "a".repeat(64);
        let sound_json =
            format!(r#"{{"source":{{"type":"custom","sha256":"{hash}"}},"volume":80}}"#);
        insert_calendar(&shared, "cal-1", Some(&sound_json));
        let set = referenced_hashes(&shared).unwrap();
        assert_eq!(set.len(), 1);
        assert!(set.contains(&hash));
    }

    #[test]
    fn referenced_hashes_collects_from_event_reminders_array() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        insert_calendar(&shared, "cal-1", None);
        let hash1 = "b".repeat(64);
        let hash2 = "c".repeat(64);
        // Two reminders, one with sound, one without — only the
        // first hash should be picked up.
        let reminders = format!(
            r#"[
                {{"kind":{{"type":"relative","minutes_before":15}},"sound":{{"source":{{"type":"custom","sha256":"{hash1}"}},"volume":80}}}},
                {{"kind":{{"type":"relative","minutes_before":5}}}}
            ]"#
        );
        insert_event_with_reminders(&shared, "ev-1", "cal-1", &reminders);
        // Second event with another reminder hash.
        let reminders2 = format!(
            r#"[{{"kind":{{"type":"app_start"}},"sound":{{"source":{{"type":"custom","sha256":"{hash2}"}},"volume":50}}}}]"#
        );
        insert_event_with_reminders(&shared, "ev-2", "cal-1", &reminders2);
        let set = referenced_hashes(&shared).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains(&hash1));
        assert!(set.contains(&hash2));
    }

    fn insert_user_pref(db: &SharedConn, key: &str, value: &str) {
        let conn = db.lock().unwrap();
        conn.execute(
            "INSERT INTO user_prefs (key, value, updated_at) VALUES (?, ?, ?)",
            params![key, value, Utc::now().to_rfc3339()],
        )
        .unwrap();
    }

    #[test]
    fn referenced_hashes_collects_from_user_prefs_overrides() {
        // §14.4 sound overrides live in user_prefs, not a DB column.
        // A custom sound referenced only there (e.g. the global
        // default) must still be pushed so other devices can fetch it.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let global_hash = "a".repeat(64);
        let cal_hash = "b".repeat(64);
        insert_user_pref(
            &shared,
            "sound.global",
            &format!(r#"{{"source":{{"type":"custom","sha256":"{global_hash}"}},"volume":80}}"#),
        );
        insert_user_pref(
            &shared,
            "sound.calendar.cal-1",
            &format!(r#"{{"source":{{"type":"custom","sha256":"{cal_hash}"}},"volume":80}}"#),
        );
        // A non-sound pref must not leak into the result set.
        insert_user_pref(&shared, "sidebar.expansion", r#"{"foo":true}"#);
        // A System-source override has no sha256 → dropped by the filter.
        insert_user_pref(
            &shared,
            "sound.item.ev-9",
            r#"{"source":{"type":"system"},"volume":80}"#,
        );
        let set = referenced_hashes(&shared).unwrap();
        assert_eq!(set.len(), 2);
        assert!(set.contains(&global_hash));
        assert!(set.contains(&cal_hash));
    }

    #[test]
    fn referenced_hashes_ignores_system_and_silent_sources() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        // System / silent sources have no sha256 field — the JSON
        // path extractor returns NULL, our WHERE drops the row.
        insert_calendar(
            &shared,
            "cal-1",
            Some(r#"{"source":{"type":"system"},"volume":80}"#),
        );
        insert_calendar(
            &shared,
            "cal-2",
            Some(r#"{"source":{"type":"silent"},"volume":80}"#),
        );
        let set = referenced_hashes(&shared).unwrap();
        assert!(set.is_empty());
    }

    #[test]
    fn list_local_sounds_skips_non_hex_filenames() {
        let tmp = TempDir::new().unwrap();
        let hash = "f".repeat(64);
        std::fs::write(tmp.path().join(format!("{hash}.mp3")), b"audio").unwrap();
        std::fs::write(tmp.path().join(".DS_Store"), b"junk").unwrap();
        std::fs::write(tmp.path().join("readme.txt"), b"docs").unwrap();
        let out = list_local_sounds(tmp.path()).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0], (hash, "mp3".to_string()));
    }

    #[test]
    fn list_local_sounds_missing_dir_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let out = list_local_sounds(&tmp.path().join("nope")).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn local_hash_present_walks_candidate_extensions() {
        let tmp = TempDir::new().unwrap();
        let hash = "a".repeat(64);
        std::fs::write(tmp.path().join(format!("{hash}.ogg")), b"audio").unwrap();
        assert_eq!(
            local_hash_present(tmp.path(), &hash),
            Some("ogg".to_string()),
        );
        let other = "b".repeat(64);
        assert_eq!(local_hash_present(tmp.path(), &other), None);
    }

    #[test]
    fn hex_digest_of_empty_is_known_sha256() {
        // SHA-256("") = e3b0c442…b855.
        assert_eq!(
            hex_digest(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        );
    }

    #[test]
    fn import_sound_writes_content_addressed_and_dedupes() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("chime.mp3");
        std::fs::write(&src, b"audio-bytes").unwrap();
        let sounds = tmp.path().join("store");
        let imported = import_sound(&sounds, &src).unwrap();
        assert_eq!(imported.ext, "mp3");
        assert_eq!(imported.sha256, hex_digest(b"audio-bytes"));
        assert!(sounds.join(format!("{}.mp3", imported.sha256)).exists());
        // Re-importing identical bytes from a differently-named source returns
        // the same hash and doesn't write a second copy.
        let src2 = tmp.path().join("other.mp3");
        std::fs::write(&src2, b"audio-bytes").unwrap();
        let again = import_sound(&sounds, &src2).unwrap();
        assert_eq!(again.sha256, imported.sha256);
        assert_eq!(list_local_sounds(&sounds).unwrap().len(), 1);
    }

    #[test]
    fn import_sound_rejects_unsupported_extension() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("notaudio.txt");
        std::fs::write(&src, b"x").unwrap();
        assert!(matches!(
            import_sound(&tmp.path().join("store"), &src),
            Err(ImportSoundError::UnsupportedFormat(_)),
        ));
    }

    #[test]
    fn import_sound_rejects_oversized() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("big.wav");
        std::fs::write(&src, vec![0u8; (MAX_SOUND_BYTES + 1) as usize]).unwrap();
        assert!(matches!(
            import_sound(&tmp.path().join("store"), &src),
            Err(ImportSoundError::TooLarge { .. }),
        ));
    }

    #[test]
    fn allowed_import_extensions_match_fetch_candidates() {
        // The import allowlist must equal what a second device probes for on
        // fetch — else an imported sound could never be fetched back.
        assert_eq!(ALLOWED_IMPORT_EXTENSIONS, FETCH_EXTENSION_CANDIDATES);
    }

    #[test]
    fn hash_was_pushed_round_trips_through_mark() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let hash = "1".repeat(64);
        assert!(!hash_was_pushed(&shared, &hash));
        mark_hash_pushed(&shared, &hash, "mp3").unwrap();
        assert!(hash_was_pushed(&shared, &hash));
    }

    #[tokio::test]
    async fn sync_assets_pushes_local_then_fetches_remote() {
        // End-to-end: device A has a local sound + a referenced
        // hash that doesn't exist locally. After one `sync_assets`
        // run, the local file is pushed to a "remote" (a temp
        // LocalFsSyncAdapter standing in for the sync store), AND
        // a different file we pre-seed on the remote is fetched.
        use sync_adapter_local::LocalFsSyncAdapter;
        let (_tmp_db, db) = fresh_db();
        let shared = db.shared();
        let local_sounds = TempDir::new().unwrap();
        let remote_dir = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(remote_dir.path().to_path_buf());

        // Pre-seed: one sound on disk locally (importer-equivalent).
        let local_hash = "a".repeat(64);
        std::fs::write(
            local_sounds.path().join(format!("{local_hash}.mp3")),
            b"local-audio",
        )
        .unwrap();

        // Pre-seed: one sound on the remote that local doesn't
        // have. Reference it from a calendar so the fetch half
        // picks it up.
        let remote_hash = "b".repeat(64);
        adapter
            .push_sound_asset(&remote_hash, "ogg", b"remote-audio")
            .await
            .unwrap();
        insert_calendar(
            &shared,
            "cal-1",
            Some(&format!(
                r#"{{"source":{{"type":"custom","sha256":"{remote_hash}"}},"volume":80}}"#
            )),
        );

        let report = sync_assets(&shared, local_sounds.path(), &adapter)
            .await
            .unwrap();
        assert_eq!(report.pushed, 1);
        assert_eq!(report.fetched, 1);
        assert_eq!(report.missing_on_remote, 0);
        assert_eq!(report.failed, 0);

        // Local file now exists on the remote.
        let on_remote = adapter.fetch_sound_asset(&local_hash, "mp3").await.unwrap();
        assert_eq!(on_remote.as_deref(), Some(b"local-audio".as_slice()));

        // Remote file now exists locally.
        let fetched_path = local_sounds.path().join(format!("{remote_hash}.ogg"));
        assert!(fetched_path.exists());
        assert_eq!(
            std::fs::read(&fetched_path).unwrap(),
            b"remote-audio".to_vec()
        );

        // `sync_assets_pushed` recorded the local hash so a
        // second pass doesn't re-push.
        assert!(hash_was_pushed(&shared, &local_hash));
        let second = sync_assets(&shared, local_sounds.path(), &adapter)
            .await
            .unwrap();
        assert_eq!(second.pushed, 0);
        assert_eq!(second.fetched, 0);
    }

    #[tokio::test]
    async fn sync_assets_handles_missing_referenced_hash_gracefully() {
        // A referenced hash that's nowhere on the remote — not
        // an error, just counted in `missing_on_remote` for the
        // call site to log.
        use sync_adapter_local::LocalFsSyncAdapter;
        let (_tmp_db, db) = fresh_db();
        let shared = db.shared();
        let local_sounds = TempDir::new().unwrap();
        let remote_dir = TempDir::new().unwrap();
        let adapter = LocalFsSyncAdapter::new(remote_dir.path().to_path_buf());

        let ghost_hash = "9".repeat(64);
        insert_calendar(
            &shared,
            "cal-1",
            Some(&format!(
                r#"{{"source":{{"type":"custom","sha256":"{ghost_hash}"}},"volume":80}}"#
            )),
        );

        let report = sync_assets(&shared, local_sounds.path(), &adapter)
            .await
            .unwrap();
        assert_eq!(report.fetched, 0);
        assert_eq!(report.missing_on_remote, 1);
        assert_eq!(report.failed, 0);
    }
}
