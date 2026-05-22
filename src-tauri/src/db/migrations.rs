//! Schema migrations.
//!
//! Each migration is `(target_version, sql)`. The runner reads the current
//! `user_version`, applies every migration with a higher target in order,
//! and bumps `user_version` inside the same transaction.
//!
//! Migrations are append-only. Never edit a published migration — add a new
//! one instead. Phase 1 only covers tables required for local calendar and
//! task CRUD; sync, FTS, plugin metadata, and event-log tables arrive in
//! their respective phases.

use super::{DbError, DbHandle, DbResult};

pub const CURRENT_SCHEMA_VERSION: u32 = 8;

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
];

pub(super) fn run(db: &DbHandle) -> DbResult<()> {
    // Read current version first; if we're already at target, do nothing
    // (no transaction, no churn).
    let current: u32 =
        db.with_conn(|c| c.query_row("PRAGMA user_version", [], |row| row.get(0)))?;

    if current >= CURRENT_SCHEMA_VERSION {
        return Ok(());
    }

    for migration in MIGRATIONS {
        if migration.target <= current {
            continue;
        }

        db.with_tx(|tx| {
            tx.execute_batch(migration.sql)
                .map_err(|e| DbError::Migration {
                    target: migration.target,
                    message: e.to_string(),
                })?;
            // PRAGMA user_version cannot be parameterised; format the
            // literal in. The value is a hard-coded constant so this is safe.
            tx.execute_batch(&format!("PRAGMA user_version = {};", migration.target))
                .map_err(|e| DbError::Migration {
                    target: migration.target,
                    message: e.to_string(),
                })?;
            Ok(())
        })?;
    }

    Ok(())
}
