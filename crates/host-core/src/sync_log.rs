//! Sync round history (DESIGN.md §19.9 "Detailliertes Sync-Protokoll").
//!
//! Storage layer for the `sync_log` table introduced in migration
//! 0014. The scheduler appends one row per attempted sync round;
//! the Settings → Synchronisation → Protokoll list reads them
//! back, newest first.
//!
//! ## Retention
//!
//! After every insert we prune to the most recent
//! [`MAX_LOG_ROWS`] entries — see the migration's header comment
//! for the rationale. Pruning runs inside the same SQLite
//! connection as the insert; the operation is a single DELETE
//! against an indexed column so the overhead is negligible (μs-
//! range even at the cap).
//!
//! ## Concurrency
//!
//! The scheduler is single-threaded by construction (one tokio
//! task per app instance), so there's no contention between
//! writers. Reads from the Tauri command layer race against the
//! background writer; both go through `SharedConn`'s mutex which
//! serialises them. No write happens inside a `.await`, so the
//! lock window is microseconds.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::db::SharedConn;

/// How many rows to keep at most. See the migration header for
/// rationale; tldr: ~3 hours of one-round-per-minute history,
/// ~60 kB on disk.
pub const MAX_LOG_ROWS: u32 = 200;

/// Why a sync round ran. Stored verbatim as the `trigger` column.
/// The frontend doesn't filter on this in v1 — it's purely for
/// the bug-report use case ("the failing rounds were all
/// periodic, manual ones work fine") — but a future filter
/// dropdown would be a one-liner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncTrigger {
    /// First sync after `APP_START_DELAY`.
    AppStart,
    /// `tokio::time::sleep(interval)` woke the worker.
    Periodic,
    /// An OS background-sync round (mobile BGTaskScheduler / WorkManager woke
    /// the app while backgrounded/closed). The desktop has no equivalent — its
    /// process-alive periodic loop already covers that window.
    Background,
    /// The EventLogWriter's debounced kick fired.
    Kick,
    /// User clicked "Sync now" in Settings or the status badge.
    Manual,
    /// Final push on `RunEvent::ExitRequested`.
    AppExit,
    /// Snapshot + log compaction ran (DESIGN.md §19.10). The
    /// resulting row's `applied` column carries the deleted-log
    /// count so the Protokoll viewer can render "N old logs
    /// removed" alongside the regular sync rounds. Compaction
    /// runs from two places — the manual `compact_now` command
    /// and the auto-trigger inside a sync round — and both
    /// land here.
    Compaction,
}

impl SyncTrigger {
    pub fn as_str(&self) -> &'static str {
        match self {
            SyncTrigger::AppStart => "app_start",
            SyncTrigger::Periodic => "periodic",
            SyncTrigger::Background => "background",
            SyncTrigger::Kick => "kick",
            SyncTrigger::Manual => "manual",
            SyncTrigger::AppExit => "app_exit",
            SyncTrigger::Compaction => "compaction",
        }
    }
}

/// One row of the sync log as read back to the frontend. Success
/// rows carry the four counter fields populated; failure rows
/// carry `error` instead. Round-trip safe via serde — the
/// Protokoll component consumes the JSON directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SyncLogEntry {
    pub id: i64,
    pub recorded_at: DateTime<Utc>,
    pub trigger: String,
    pub success: bool,
    pub pushed_logs: Option<u32>,
    pub fetched_logs: Option<u32>,
    pub applied: Option<u32>,
    pub conflicts: Option<u32>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
}

/// Insert-side counters. `None` everywhere means "round failed
/// before any phase produced numbers"; in practice the
/// orchestrator only fails the whole round when the adapter
/// rejects at step zero, so most failures still carry partial
/// counts (e.g. push succeeded → pushed_logs is `Some`).
#[derive(Debug, Clone, Default)]
pub struct SyncLogCounters {
    pub pushed_logs: Option<u32>,
    pub fetched_logs: Option<u32>,
    pub applied: Option<u32>,
    pub conflicts: Option<u32>,
}

#[derive(Debug, Error)]
pub enum SyncLogError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub type SyncLogResult<T> = Result<T, SyncLogError>;

/// SQLite-backed repository for `sync_log`. Thin wrapper: no
/// policy lives here, just CRUD.
pub struct SyncLogRepo<'a> {
    db: &'a SharedConn,
}

impl<'a> SyncLogRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Append one sync-round entry + prune to the retention cap.
    /// Returns the new row id so the scheduler can correlate the
    /// insert with the `sync-log-changed` event it emits to
    /// trigger a frontend refresh.
    pub fn record(
        &self,
        trigger: SyncTrigger,
        success: bool,
        counters: &SyncLogCounters,
        duration_ms: Option<u64>,
        error: Option<&str>,
    ) -> SyncLogResult<i64> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO sync_log (
                recorded_at, trigger, success,
                pushed_logs, fetched_logs, applied, conflicts,
                duration_ms, error
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                Utc::now().to_rfc3339(),
                trigger.as_str(),
                if success { 1 } else { 0 },
                counters.pushed_logs,
                counters.fetched_logs,
                counters.applied,
                counters.conflicts,
                duration_ms,
                error,
            ],
        )?;
        let id = conn.last_insert_rowid();

        // Prune to the retention cap. The subquery picks the oldest
        // rows by recorded_at; the outer DELETE drops them. Doing
        // it here (rather than in a separate scheduled task) keeps
        // the table bounded in size even on long-running sessions
        // without spinning a second tokio task.
        conn.execute(
            "DELETE FROM sync_log
              WHERE id IN (
                  SELECT id FROM sync_log
                   ORDER BY recorded_at DESC
                   LIMIT -1 OFFSET ?
              )",
            params![MAX_LOG_ROWS],
        )?;
        Ok(id)
    }

    /// Read entries newest-first. `limit` caps the returned set;
    /// values above [`MAX_LOG_ROWS`] are still honoured (rows
    /// beyond the cap just don't exist).
    pub fn list(&self, limit: u32) -> SyncLogResult<Vec<SyncLogEntry>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, recorded_at, trigger, success,
                    pushed_logs, fetched_logs, applied, conflicts,
                    duration_ms, error
               FROM sync_log
              ORDER BY recorded_at DESC
              LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit], row_to_entry)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Drop every row. The Settings → Protokoll "Verlauf leeren"
    /// button calls this; useful for "I'm about to share my
    /// screen, scrub the history" scenarios.
    pub fn clear(&self) -> SyncLogResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute("DELETE FROM sync_log", [])?;
        Ok(())
    }
}

/// Convert one SQLite row into a [`SyncLogEntry`]. Mirrors the
/// column order of the SELECT statements in [`SyncLogRepo`].
fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<SyncLogResult<SyncLogEntry>> {
    let id: i64 = row.get(0)?;
    let recorded_at_raw: String = row.get(1)?;
    let trigger: String = row.get(2)?;
    let success_raw: i64 = row.get(3)?;
    let pushed_logs: Option<u32> = row.get(4)?;
    let fetched_logs: Option<u32> = row.get(5)?;
    let applied: Option<u32> = row.get(6)?;
    let conflicts: Option<u32> = row.get(7)?;
    let duration_ms: Option<u64> = row.get(8)?;
    let error: Option<String> = row.get(9)?;
    let recorded_at = match DateTime::parse_from_rfc3339(&recorded_at_raw) {
        Ok(dt) => dt.with_timezone(&Utc),
        Err(err) => {
            // Stored bad timestamp → surface as a Sqlite-shaped
            // error rather than panicking. In practice the only
            // writer is our own `record()` which always writes
            // RFC3339, so this is defensive.
            return Ok(Err(SyncLogError::Sqlite(
                rusqlite::Error::FromSqlConversionFailure(
                    1,
                    rusqlite::types::Type::Text,
                    Box::new(err),
                ),
            )));
        }
    };
    Ok(Ok(SyncLogEntry {
        id,
        recorded_at,
        trigger,
        success: success_raw != 0,
        pushed_logs,
        fetched_logs,
        applied,
        conflicts,
        duration_ms,
        error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let db = DbHandle::open(dir.path().join("test.sqlite")).unwrap();
        (dir, db)
    }

    #[test]
    fn record_then_list_roundtrips() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = SyncLogRepo::new(&shared);
        let counters = SyncLogCounters {
            pushed_logs: Some(3),
            fetched_logs: Some(5),
            applied: Some(12),
            conflicts: Some(0),
        };
        repo.record(SyncTrigger::Periodic, true, &counters, Some(420), None)
            .unwrap();
        let entries = repo.list(50).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert!(e.success);
        assert_eq!(e.trigger, "periodic");
        assert_eq!(e.pushed_logs, Some(3));
        assert_eq!(e.fetched_logs, Some(5));
        assert_eq!(e.applied, Some(12));
        assert_eq!(e.conflicts, Some(0));
        assert_eq!(e.duration_ms, Some(420));
        assert_eq!(e.error, None);
    }

    #[test]
    fn failure_row_carries_error_and_null_counters() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = SyncLogRepo::new(&shared);
        repo.record(
            SyncTrigger::Manual,
            false,
            &SyncLogCounters::default(),
            Some(15),
            Some("connection refused"),
        )
        .unwrap();
        let entries = repo.list(10).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        assert!(!e.success);
        assert_eq!(e.error.as_deref(), Some("connection refused"));
        assert_eq!(e.pushed_logs, None);
    }

    #[test]
    fn list_is_newest_first() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = SyncLogRepo::new(&shared);
        repo.record(
            SyncTrigger::AppStart,
            true,
            &SyncLogCounters::default(),
            None,
            None,
        )
        .unwrap();
        // Sleep just long enough for the next RFC3339 timestamp to
        // sort strictly after the previous one. Millisecond
        // precision is the rfc3339 floor we use.
        std::thread::sleep(std::time::Duration::from_millis(20));
        repo.record(
            SyncTrigger::Manual,
            true,
            &SyncLogCounters::default(),
            None,
            None,
        )
        .unwrap();
        let entries = repo.list(10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].trigger, "manual");
        assert_eq!(entries[1].trigger, "app_start");
    }

    #[test]
    fn prune_caps_table_at_max_rows() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = SyncLogRepo::new(&shared);
        // Insert MAX_LOG_ROWS + 5 entries.
        for _ in 0..(MAX_LOG_ROWS + 5) {
            repo.record(
                SyncTrigger::Periodic,
                true,
                &SyncLogCounters::default(),
                None,
                None,
            )
            .unwrap();
        }
        // List with a generous limit. The cap should hold even
        // though we asked for more.
        let entries = repo.list(MAX_LOG_ROWS + 100).unwrap();
        assert_eq!(entries.len(), MAX_LOG_ROWS as usize);
    }

    #[test]
    fn clear_drops_everything() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = SyncLogRepo::new(&shared);
        for _ in 0..5 {
            repo.record(
                SyncTrigger::Periodic,
                true,
                &SyncLogCounters::default(),
                None,
                None,
            )
            .unwrap();
        }
        assert_eq!(repo.list(50).unwrap().len(), 5);
        repo.clear().unwrap();
        assert!(repo.list(50).unwrap().is_empty());
    }
}
