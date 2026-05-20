//! Account model and persistence (DESIGN.md §6.2 + §6.4).
//!
//! An *account* is one user-configured instance of an adapter: the
//! adapter kind ("local", "caldav", "google", …), a human-readable
//! display name, and a small JSON config blob that holds the
//! adapter-specific non-secret configuration (server URL, OAuth
//! client id, calendar root path, …). Secrets live in the platform
//! keychain via the `secrets` module.
//!
//! This phase only models the storage layer + the account CRUD
//! commands. The actual `Adapter` instances per account, and the
//! aggregation of multiple adapters into the existing
//! calendar/event commands, arrive in Phase 6b when the first
//! external adapter (CalDAV) lands and there is a meaningful
//! "more than one adapter" scenario to route over.

use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::db::SharedConn;

/// The hard-coded account id of the implicit local adapter. Matches
/// `cal-adapter-local::SOURCE_ID` so existing rows that carry
/// `source = "local"` can be attached without rewriting them.
pub const LOCAL_ACCOUNT_ID: &str = "local";

/// Adapter kinds Aperio knows how to construct. Listed exhaustively
/// so the frontend can show each option's status (available vs
/// "coming in Phase 6b/6d") and the backend can refuse unknown
/// kinds at the boundary rather than failing at adapter construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterKind {
    Local,
    Caldav,
    Ical,
    Google,
    MicrosoftGraph,
    Ews,
    Vikunja,
    Todoist,
}

impl AdapterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AdapterKind::Local => "local",
            AdapterKind::Caldav => "caldav",
            AdapterKind::Ical => "ical",
            AdapterKind::Google => "google",
            AdapterKind::MicrosoftGraph => "microsoft_graph",
            AdapterKind::Ews => "ews",
            AdapterKind::Vikunja => "vikunja",
            AdapterKind::Todoist => "todoist",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "local" => AdapterKind::Local,
            "caldav" => AdapterKind::Caldav,
            "ical" => AdapterKind::Ical,
            "google" => AdapterKind::Google,
            "microsoft_graph" => AdapterKind::MicrosoftGraph,
            "ews" => AdapterKind::Ews,
            "vikunja" => AdapterKind::Vikunja,
            "todoist" => AdapterKind::Todoist,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub adapter_kind: AdapterKind,
    pub display_name: String,
    /// Adapter-specific non-secret config (server URL, …). Stored
    /// verbatim as a JSON string so this module doesn't need to know
    /// the shape of every adapter's config.
    pub config_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Error)]
pub enum AccountsError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("unknown adapter kind: {0}")]
    UnknownKind(String),
    #[error("account '{0}' not found")]
    NotFound(String),
    #[error("cannot delete the implicit local account")]
    DeleteLocalForbidden,
}

/// Read-side access to the `accounts` table. Stateless — every
/// operation acquires the connection lock from the shared handle.
pub struct AccountsRepo<'a> {
    pub(crate) db: &'a SharedConn,
}

impl<'a> AccountsRepo<'a> {
    pub fn new(db: &'a SharedConn) -> Self {
        Self { db }
    }

    pub fn list(&self) -> Result<Vec<Account>, AccountsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, adapter_kind, display_name, config_json,
                    created_at, updated_at
               FROM accounts
              ORDER BY adapter_kind, display_name COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], row_to_account)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r??);
        }
        Ok(out)
    }

    pub fn get(&self, id: &str) -> Result<Option<Account>, AccountsError> {
        let conn = self.db.lock().expect("db mutex poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, adapter_kind, display_name, config_json,
                    created_at, updated_at
               FROM accounts WHERE id = ?",
        )?;
        let row = stmt
            .query_row(params![id], row_to_account)
            .map_err(|err| match err {
                rusqlite::Error::QueryReturnedNoRows => {
                    AccountsError::NotFound(id.to_string())
                }
                other => AccountsError::Sqlite(other),
            });
        match row {
            Ok(res) => res.map(Some),
            Err(AccountsError::NotFound(_)) => Ok(None),
            Err(err) => Err(err),
        }
    }

    pub fn create(
        &self,
        adapter_kind: AdapterKind,
        display_name: &str,
        config_json: &str,
    ) -> Result<Account, AccountsError> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let conn = self.db.lock().expect("db mutex poisoned");
        conn.execute(
            "INSERT INTO accounts (id, adapter_kind, display_name,
                                   config_json, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
            params![id, adapter_kind.as_str(), display_name, config_json, now, now],
        )?;
        Ok(Account {
            id,
            adapter_kind,
            display_name: display_name.to_string(),
            config_json: config_json.to_string(),
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn delete(&self, id: &str) -> Result<(), AccountsError> {
        if id == LOCAL_ACCOUNT_ID {
            return Err(AccountsError::DeleteLocalForbidden);
        }
        let conn = self.db.lock().expect("db mutex poisoned");
        let changed = conn.execute("DELETE FROM accounts WHERE id = ?", params![id])?;
        if changed == 0 {
            return Err(AccountsError::NotFound(id.to_string()));
        }
        Ok(())
    }
}

fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Account, AccountsError>> {
    let id: String = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let display_name: String = row.get(2)?;
    let config_json: String = row.get(3)?;
    let created_at: String = row.get(4)?;
    let updated_at: String = row.get(5)?;
    let Some(adapter_kind) = AdapterKind::parse(&kind_str) else {
        return Ok(Err(AccountsError::UnknownKind(kind_str)));
    };
    Ok(Ok(Account {
        id,
        adapter_kind,
        display_name,
        config_json,
        created_at,
        updated_at,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbHandle;
    use tempfile::TempDir;

    fn fresh_db() -> (TempDir, DbHandle) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.sqlite");
        let db = DbHandle::open(&path).unwrap();
        (dir, db)
    }

    #[test]
    fn local_account_is_seeded_by_migration() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let local = repo.get(LOCAL_ACCOUNT_ID).unwrap();
        let local = local.expect("local account should be seeded");
        assert_eq!(local.adapter_kind, AdapterKind::Local);
        assert_eq!(local.display_name, "Local");
    }

    #[test]
    fn create_list_delete_roundtrip() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);

        let before = repo.list().unwrap();
        let initial_count = before.len();

        let new = repo
            .create(AdapterKind::Caldav, "Nextcloud (home)", "{}")
            .unwrap();
        let after = repo.list().unwrap();
        assert_eq!(after.len(), initial_count + 1);
        assert!(after.iter().any(|a| a.id == new.id));

        repo.delete(&new.id).unwrap();
        assert!(repo.get(&new.id).unwrap().is_none());
    }

    #[test]
    fn deleting_local_is_refused() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let err = repo.delete(LOCAL_ACCOUNT_ID).unwrap_err();
        assert!(matches!(err, AccountsError::DeleteLocalForbidden));
    }
}
