//! Aperio's local SQLite schema + migration runner.
//!
//! The schema (one append-only `.sql` file per version under `src/sql/`)
//! and the `user_version`-tracked runner live here so the desktop backend
//! (`src-tauri`) and the mobile app share one source of truth: each opens
//! its own [`rusqlite::Connection`] (desktop via the file DB + read pool,
//! mobile in the app sandbox), then calls [`run`] to bring it up to
//! [`CURRENT_SCHEMA_VERSION`]. The higher layers (`cal-adapter-local`,
//! the sync engine) only ever operate on an already-migrated connection.
//!
//! Migrations are append-only. Never edit a published migration — add a
//! new `.sql` file + `Migration` entry instead.

use rusqlite::Connection;
use thiserror::Error;

/// The schema version the current code expects. [`run`] applies every
/// migration up to and including this one.
pub const CURRENT_SCHEMA_VERSION: u32 = 28;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("migration to schema version {target} failed: {message}")]
    Failed { target: u32, message: String },
}

struct Migration {
    target: u32,
    sql: &'static str,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        target: 1,
        sql: include_str!("sql/0001_init.sql"),
    },
    Migration {
        target: 2,
        sql: include_str!("sql/0002_search.sql"),
    },
    Migration {
        target: 3,
        sql: include_str!("sql/0003_accounts.sql"),
    },
    Migration {
        target: 4,
        sql: include_str!("sql/0004_container_overrides.sql"),
    },
    Migration {
        target: 5,
        sql: include_str!("sql/0005_user_prefs.sql"),
    },
    Migration {
        target: 6,
        sql: include_str!("sql/0006_task_time_fields.sql"),
    },
    Migration {
        target: 7,
        sql: include_str!("sql/0007_contacts.sql"),
    },
    Migration {
        target: 8,
        sql: include_str!("sql/0008_contacts_fts.sql"),
    },
    Migration {
        target: 9,
        sql: include_str!("sql/0009_contact_members.sql"),
    },
    Migration {
        target: 10,
        sql: include_str!("sql/0010_contact_photos.sql"),
    },
    Migration {
        target: 11,
        sql: include_str!("sql/0011_contact_addresses.sql"),
    },
    Migration {
        target: 12,
        sql: include_str!("sql/0012_sync_applied_events.sql"),
    },
    Migration {
        target: 13,
        sql: include_str!("sql/0013_sync_conflicts.sql"),
    },
    Migration {
        target: 14,
        sql: include_str!("sql/0014_sync_log.sql"),
    },
    Migration {
        target: 15,
        sql: include_str!("sql/0015_sync_assets_pushed.sql"),
    },
    Migration {
        target: 16,
        sql: include_str!("sql/0016_remote_plugins.sql"),
    },
    Migration {
        target: 17,
        sql: include_str!("sql/0017_device_names.sql"),
    },
    Migration {
        target: 18,
        sql: include_str!("sql/0018_task_sections.sql"),
    },
    Migration {
        target: 19,
        sql: include_str!("sql/0019_external_cache.sql"),
    },
    Migration {
        target: 20,
        sql: include_str!("sql/0020_cache_native_id.sql"),
    },
    Migration {
        target: 21,
        sql: include_str!("sql/0021_recurring_cache_rewarm.sql"),
    },
    Migration {
        target: 22,
        sql: include_str!("sql/0022_container_color_labels.sql"),
    },
    Migration {
        target: 23,
        sql: include_str!("sql/0023_color_label_ad_hoc.sql"),
    },
    Migration {
        target: 24,
        sql: include_str!("sql/0024_section_color.sql"),
    },
    Migration {
        target: 25,
        sql: include_str!("sql/0025_section_color_overrides.sql"),
    },
    Migration {
        target: 26,
        sql: include_str!("sql/0026_event_color_overrides.sql"),
    },
    Migration {
        target: 27,
        sql: include_str!("sql/0027_cache_search.sql"),
    },
    Migration {
        target: 28,
        sql: include_str!("sql/0028_task_resurface_series.sql"),
    },
];

/// Bring `conn` up to [`CURRENT_SCHEMA_VERSION`] by applying every
/// migration whose target exceeds the connection's current
/// `user_version`, each inside its own transaction (so a mid-run failure
/// leaves the schema at the last good version, never half-applied). A
/// no-op when the connection is already current.
pub fn run(conn: &mut Connection) -> Result<(), MigrationError> {
    // Read the current version first; if we're already at target, do
    // nothing (no transaction, no churn).
    let current: u32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if current >= CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    for migration in MIGRATIONS {
        if migration.target <= current {
            continue;
        }

        let tx = conn.transaction()?;
        tx.execute_batch(migration.sql)
            .map_err(|e| MigrationError::Failed {
                target: migration.target,
                message: e.to_string(),
            })?;
        // PRAGMA user_version cannot be parameterised; format the literal
        // in. The value is a hard-coded constant so this is safe.
        tx.execute_batch(&format!("PRAGMA user_version = {};", migration.target))
            .map_err(|e| MigrationError::Failed {
                target: migration.target,
                message: e.to_string(),
            })?;
        tx.commit()?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    #[test]
    fn run_brings_a_fresh_db_to_current_version() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        run(&mut conn).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn run_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        // Running again on the same connection must not error or move the
        // version.
        run(&mut conn).unwrap();
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    /// Migration 0006 collapses the old `deadline_type` enum into the
    /// new (scheduled, deadline) pair. The data-preservation rules are
    /// documented in the migration header; this test pins all four
    /// branches in one shot.
    #[test]
    fn migration_0006_preserves_legacy_task_dates() {
        // To exercise the migration on real legacy rows we build a
        // snapshot DB at v5 (apply 0001…0005 explicitly), seed the
        // legacy shape, then run 0006 and inspect the output.
        let legacy = Connection::open_in_memory().unwrap();
        legacy.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        legacy
            .execute_batch(include_str!("sql/0001_init.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0002_search.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0003_accounts.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0004_container_overrides.sql"))
            .unwrap();
        legacy
            .execute_batch(include_str!("sql/0005_user_prefs.sql"))
            .unwrap();

        // Migration 0003 already seeds the 'local' account row; skip
        // re-inserting (the SQL we execute_batch'd ran every step).
        legacy
            .execute(
                "INSERT INTO task_lists (id, name, source, account_id, created_at, updated_at)
                 VALUES ('L1', 'List', 'local', 'local',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();

        // Four rows covering each branch of the migration.
        let mk = |id: &str,
                  sd: Option<&str>,
                  dt: Option<&str>,
                  dd: Option<&str>,
                  dtime: Option<&str>| {
            legacy
                .execute(
                    "INSERT INTO tasks (
                        id, list_id, title, status, priority,
                        scheduled_date, deadline_type, deadline_date, deadline_time,
                        created_at, updated_at
                     ) VALUES (?, 'L1', ?, 'open', 'medium', ?, ?, ?, ?,
                               '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
                    params![id, id, sd, dt, dd, dtime],
                )
                .unwrap();
        };

        // a: legacy "by" deadline — keep deadline as-is.
        mk("a", None, Some("by"), Some("2026-07-31"), None);
        // b: legacy "on" with no scheduled — move to scheduled.
        mk("b", None, Some("on"), Some("2026-08-15"), Some("14:30:00"));
        // c: legacy "on" with scheduled = same day — keep schedule, clear deadline.
        mk(
            "c",
            Some("2026-09-01"),
            Some("on"),
            Some("2026-09-01"),
            None,
        );
        // d: legacy "on" with scheduled = different day — keep both,
        // deadline degrades to "by".
        mk(
            "d",
            Some("2026-10-05"),
            Some("on"),
            Some("2026-10-10"),
            Some("17:00:00"),
        );

        // Apply 0006.
        legacy
            .execute_batch(include_str!("sql/0006_task_time_fields.sql"))
            .unwrap();

        let read = |id: &str| -> (
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) {
            legacy
                .query_row(
                    "SELECT scheduled_date, scheduled_time, deadline_date, deadline_time
                       FROM tasks WHERE id = ?",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .unwrap()
        };

        // a — "by" preserved verbatim, schedule untouched.
        assert_eq!(
            read("a"),
            (None, None, Some("2026-07-31".to_string()), None)
        );
        // b — "on" without schedule → schedule takes the date+time.
        assert_eq!(
            read("b"),
            (
                Some("2026-08-15".to_string()),
                Some("14:30:00".to_string()),
                None,
                None
            )
        );
        // c — "on" same day as schedule → schedule kept, deadline cleared.
        assert_eq!(
            read("c"),
            (Some("2026-09-01".to_string()), None, None, None)
        );
        // d — "on" different day → both preserved (Plan + Soft-Deadline).
        assert_eq!(
            read("d"),
            (
                Some("2026-10-05".to_string()),
                None,
                Some("2026-10-10".to_string()),
                Some("17:00:00".to_string())
            )
        );
    }
}
