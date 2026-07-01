//! Sync-conflict repository (DESIGN.md §19.3, Phase Sh).
//!
//! Storage layer for the `sync_conflicts` table introduced in
//! migration 0013. The applier writes a row here every time it
//! detects a true field-level divergence; the Tauri command layer
//! reads them out + dispatches the user's resolution choice.
//!
//! See [`ConflictRecord`] for the full row shape. The repo is a
//! thin SQLite wrapper — no business logic lives here, just
//! type-safe CRUD on the table.

use chrono::{DateTime, Utc};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::db::SharedConn;

/// `ConflictKind` (which synchronisable table a conflict belongs to) and
/// its `as_str`/`from_str` round-trip are defined in the `sync-engine`
/// crate — the applier, which lives there now, records conflicts with it.
/// Re-exported here because this module owns the `sync_conflicts` table it
/// serialises into. The stored/read shape ([`ConflictRecord`]) and the
/// repository stay desktop-side.
pub use sync_engine::ConflictKind;

/// One pending or resolved conflict. Both `local_value` and
/// `remote_value` are JSON-encoded — the frontend renders them
/// via the same component the EventDialog / TaskDialog use for
/// the matching field type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictRecord {
    pub id: i64,
    pub detected_at: DateTime<Utc>,
    pub row_kind: ConflictKind,
    pub row_id: String,
    pub field: String,
    pub local_value: Option<String>,
    pub remote_value: Option<String>,
    pub remote_device_id: String,
    pub remote_timestamp: DateTime<Utc>,
    pub resolved: bool,
    pub resolution: Option<ResolutionChoice>,
    pub resolved_at: Option<DateTime<Utc>>,
}

/// Choice the user picks in the §19.3 dialog. Stored as the
/// `resolution` column so we have an audit trail of who decided
/// what.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionChoice {
    /// "Meine Version behalten" — no data change.
    KeepLocal,
    /// "Andere Version nehmen" — apply the remote value locally.
    TakeRemote,
    /// "Beide als separate Termine speichern" — fork the row;
    /// implemented as a fresh `*Created` event with the remote
    /// field swapped in.
    SaveBoth,
}

impl ResolutionChoice {
    pub fn as_str(&self) -> &'static str {
        match self {
            ResolutionChoice::KeepLocal => "keep_local",
            ResolutionChoice::TakeRemote => "take_remote",
            ResolutionChoice::SaveBoth => "save_both",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "keep_local" => Some(ResolutionChoice::KeepLocal),
            "take_remote" => Some(ResolutionChoice::TakeRemote),
            "save_both" => Some(ResolutionChoice::SaveBoth),
            _ => None,
        }
    }
}

/// Insert-side shape — defined in `sync-engine` (the applier builds it
/// there when it can't auto-merge a field). Same fields as
/// [`ConflictRecord`] minus the SQLite-assigned `id` and the resolution
/// columns. Re-exported so `ConflictsRepo::record` keeps its signature.
pub use sync_engine::NewConflict;

#[derive(Debug, Error)]
pub enum ConflictsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("invalid conflict kind: {0}")]
    InvalidKind(String),
    #[error("invalid resolution choice: {0}")]
    InvalidResolution(String),
    #[error("conflict {0} not found")]
    NotFound(i64),
}

pub type ConflictsResult<T> = Result<T, ConflictsError>;

/// SQLite-backed repository for the `sync_conflicts` table.
///
/// Thin — no policy decisions live here. The applier calls
/// `record` when it can't auto-merge; the command layer calls
/// `list_unresolved` to render the dialog and `mark_resolved` /
/// `delete` to finalise the user's choice.
pub struct ConflictsRepo<'a> {
    db: &'a SharedConn,
}

impl<'a> ConflictsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    /// Insert a fresh conflict (or supersede the existing
    /// unresolved one for the same (kind, id, field) — the
    /// partial UNIQUE INDEX in migration 0013 handles the
    /// supersede automatically via INSERT OR REPLACE-style
    /// semantics, but we do it explicitly so the SQL stays
    /// readable.
    pub fn record(&self, c: NewConflict) -> ConflictsResult<i64> {
        let conn = self.db.lock().expect("db mutex poisoned");
        // First clear any prior unresolved conflict on the same
        // field — the user resolves the latest divergence, not
        // every intermediate one. Without this, the UNIQUE INDEX
        // would reject the insert.
        conn.execute(
            "DELETE FROM sync_conflicts
              WHERE row_kind = ? AND row_id = ? AND field = ? AND resolved = 0",
            params![c.row_kind.as_str(), c.row_id, c.field],
        )?;
        conn.execute(
            "INSERT INTO sync_conflicts (
                detected_at, row_kind, row_id, field,
                local_value, remote_value,
                remote_device_id, remote_timestamp,
                resolved
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)",
            params![
                Utc::now().to_rfc3339(),
                c.row_kind.as_str(),
                c.row_id,
                c.field,
                c.local_value,
                c.remote_value,
                c.remote_device_id,
                c.remote_timestamp.to_rfc3339(),
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Read every unresolved conflict, newest first. Used by the
    /// `list_sync_conflicts` Tauri command — typically called
    /// once on app start + after every `sync-conflicts-changed`
    /// event the applier emits.
    pub fn list_unresolved(&self) -> ConflictsResult<Vec<ConflictRecord>> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, detected_at, row_kind, row_id, field,
                    local_value, remote_value,
                    remote_device_id, remote_timestamp,
                    resolved, resolution, resolved_at
               FROM sync_conflicts
              WHERE resolved = 0
              ORDER BY detected_at DESC",
        )?;
        let rows = stmt.query_map([], row_to_conflict)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    /// Auto-resolve stored conflicts that a newer build no longer considers
    /// genuine — a `recurrence` serialization-drift conflict recorded before
    /// `canonicalize_recurrence` existed, or a metadata field. The applier's
    /// live check only guards NEW envelopes, so without this a pre-fix spurious
    /// conflict lingers in the dialog forever after the user updates. Idempotent
    /// (once pruned there's nothing left to re-check). Clears with `KeepLocal` —
    /// a pure-bookkeeping no-op, since the two values are equal under today's
    /// rules. Returns how many were pruned. Real divergences are untouched.
    pub fn prune_stale(&self) -> ConflictsResult<usize> {
        let parse = |v: &Option<String>| {
            v.as_deref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .unwrap_or(serde_json::Value::Null)
        };
        let mut pruned = 0;
        for c in self.list_unresolved()? {
            if !sync_engine::conflict_still_genuine(
                c.row_kind,
                &c.field,
                &parse(&c.local_value),
                &parse(&c.remote_value),
            ) {
                self.mark_resolved(c.id, ResolutionChoice::KeepLocal)?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    /// Fetch one conflict by id. Returns `Err(NotFound)` when no
    /// row matches — the command layer translates that to a
    /// "stale dialog" message.
    pub fn get(&self, id: i64) -> ConflictsResult<ConflictRecord> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, detected_at, row_kind, row_id, field,
                    local_value, remote_value,
                    remote_device_id, remote_timestamp,
                    resolved, resolution, resolved_at
               FROM sync_conflicts
              WHERE id = ?",
        )?;

        stmt.query_row(params![id], row_to_conflict)
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => ConflictsError::NotFound(id),
                other => ConflictsError::Sqlite(other),
            })?
    }

    /// Flip `resolved = 1` and record the choice + timestamp.
    /// The caller has already executed whatever side-effect the
    /// choice implies (no-op for `KeepLocal`, an upsert for
    /// `TakeRemote`, a clone for `SaveBoth`).
    pub fn mark_resolved(&self, id: i64, choice: ResolutionChoice) -> ConflictsResult<()> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let affected = conn.execute(
            "UPDATE sync_conflicts
                SET resolved = 1,
                    resolution = ?,
                    resolved_at = ?
              WHERE id = ?",
            params![choice.as_str(), Utc::now().to_rfc3339(), id,],
        )?;
        if affected == 0 {
            return Err(ConflictsError::NotFound(id));
        }
        Ok(())
    }

    /// Count of unresolved rows. Used by the status indicator so
    /// the badge can flip on without loading the full list.
    pub fn unresolved_count(&self) -> ConflictsResult<usize> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sync_conflicts WHERE resolved = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n as usize)
    }
}

fn row_to_conflict(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConflictsResult<ConflictRecord>> {
    let id: i64 = row.get(0)?;
    let detected_at_raw: String = row.get(1)?;
    let row_kind_raw: String = row.get(2)?;
    let row_id: String = row.get(3)?;
    let field: String = row.get(4)?;
    let local_value: Option<String> = row.get(5)?;
    let remote_value: Option<String> = row.get(6)?;
    let remote_device_id: String = row.get(7)?;
    let remote_timestamp_raw: String = row.get(8)?;
    let resolved_int: i64 = row.get(9)?;
    let resolution_raw: Option<String> = row.get(10)?;
    let resolved_at_raw: Option<String> = row.get(11)?;
    Ok((|| -> ConflictsResult<ConflictRecord> {
        let row_kind = ConflictKind::from_str(&row_kind_raw)
            .ok_or_else(|| ConflictsError::InvalidKind(row_kind_raw.clone()))?;
        let resolution = match resolution_raw {
            Some(s) => Some(
                ResolutionChoice::from_str(&s)
                    .ok_or_else(|| ConflictsError::InvalidResolution(s))?,
            ),
            None => None,
        };
        Ok(ConflictRecord {
            id,
            detected_at: parse_ts(&detected_at_raw)?,
            row_kind,
            row_id,
            field,
            local_value,
            remote_value,
            remote_device_id,
            remote_timestamp: parse_ts(&remote_timestamp_raw)?,
            resolved: resolved_int != 0,
            resolution,
            resolved_at: resolved_at_raw.map(|s| parse_ts(&s)).transpose()?,
        })
    })())
}

fn parse_ts(s: &str) -> ConflictsResult<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|d| d.with_timezone(&Utc))
        .map_err(|err| {
            ConflictsError::Sqlite(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                Box::new(err),
            ))
        })
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

    fn fake_conflict(field: &str) -> NewConflict {
        NewConflict {
            row_kind: ConflictKind::Event,
            row_id: "ev-1".into(),
            field: field.into(),
            local_value: Some("\"local\"".into()),
            remote_value: Some("\"remote\"".into()),
            remote_device_id: "dev-other".into(),
            remote_timestamp: Utc::now(),
        }
    }

    #[test]
    fn record_then_list_returns_one_row() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        repo.record(fake_conflict("title")).unwrap();
        let rows = repo.list_unresolved().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "title");
        assert_eq!(rows[0].row_kind, ConflictKind::Event);
        assert!(!rows[0].resolved);
    }

    #[test]
    fn second_conflict_on_same_field_supersedes_prior() {
        // The UNIQUE INDEX is partial on `resolved = 0`; the
        // repo's `record` clears any prior unresolved row before
        // inserting the new one. Net effect: only one unresolved
        // conflict per (kind, id, field).
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        repo.record(NewConflict {
            local_value: Some("\"old-local\"".into()),
            remote_value: Some("\"old-remote\"".into()),
            ..fake_conflict("title")
        })
        .unwrap();
        repo.record(NewConflict {
            local_value: Some("\"new-local\"".into()),
            remote_value: Some("\"new-remote\"".into()),
            ..fake_conflict("title")
        })
        .unwrap();
        let rows = repo.list_unresolved().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].local_value.as_deref(), Some("\"new-local\""));
    }

    #[test]
    fn prune_stale_drops_spurious_recurrence_but_keeps_real_conflict() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        // Spurious: the SAME daily recurrence, but the remote payload (older
        // build) omits the §9.12 default axes the current model always emits.
        // These are the exact shapes from a real cross-device conflict.
        repo.record(NewConflict {
            row_kind: ConflictKind::Task,
            row_id: "task-1".into(),
            field: "recurrence".into(),
            local_value: Some(
                r#"{"anchor":"from_date","day_of_month":null,"day_of_week":null,"end":{"type":"never"},"fixed_dates":null,"frequency":"daily","interval":1,"placement":"schedule"}"#
                    .into(),
            ),
            remote_value: Some(
                r#"{"day_of_month":null,"day_of_week":null,"end":{"type":"never"},"frequency":"daily","interval":1}"#
                    .into(),
            ),
            remote_device_id: "dev-other".into(),
            remote_timestamp: Utc::now(),
        })
        .unwrap();
        // Real: genuinely different status — must survive the prune.
        repo.record(NewConflict {
            row_kind: ConflictKind::Task,
            row_id: "task-1".into(),
            field: "status".into(),
            local_value: Some("\"completed\"".into()),
            remote_value: Some("\"open\"".into()),
            remote_device_id: "dev-other".into(),
            remote_timestamp: Utc::now(),
        })
        .unwrap();

        assert_eq!(repo.unresolved_count().unwrap(), 2);
        assert_eq!(repo.prune_stale().unwrap(), 1, "only the recurrence one");
        let rows = repo.list_unresolved().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].field, "status", "the real conflict stays");
        // Idempotent — a second pass finds nothing to prune.
        assert_eq!(repo.prune_stale().unwrap(), 0);
    }

    #[test]
    fn mark_resolved_takes_row_out_of_unresolved_list() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        let id = repo.record(fake_conflict("title")).unwrap();
        assert_eq!(repo.unresolved_count().unwrap(), 1);
        repo.mark_resolved(id, ResolutionChoice::KeepLocal).unwrap();
        assert_eq!(repo.unresolved_count().unwrap(), 0);
        // The resolved row stays available via direct fetch.
        let row = repo.get(id).unwrap();
        assert!(row.resolved);
        assert_eq!(row.resolution, Some(ResolutionChoice::KeepLocal));
    }

    #[test]
    fn unresolved_count_excludes_resolved() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        let id1 = repo.record(fake_conflict("title")).unwrap();
        let _id2 = repo.record(fake_conflict("location")).unwrap();
        repo.mark_resolved(id1, ResolutionChoice::TakeRemote)
            .unwrap();
        assert_eq!(repo.unresolved_count().unwrap(), 1);
    }

    #[test]
    fn resolved_row_does_not_block_new_conflict_on_same_field() {
        // Partial UNIQUE INDEX is filtered by `resolved = 0`, so
        // a resolved row from earlier shouldn't reject a fresh
        // conflict.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        let id1 = repo.record(fake_conflict("title")).unwrap();
        repo.mark_resolved(id1, ResolutionChoice::KeepLocal)
            .unwrap();
        // Same field, fresh divergence:
        repo.record(fake_conflict("title")).unwrap();
        assert_eq!(repo.unresolved_count().unwrap(), 1);
    }

    #[test]
    fn get_on_missing_id_returns_not_found() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = ConflictsRepo::new(&shared);
        let err = repo.get(9999).unwrap_err();
        assert!(matches!(err, ConflictsError::NotFound(9999)));
    }
}
