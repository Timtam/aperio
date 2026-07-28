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
    /// Device-local calendar + reminders (iOS EventKit / Android
    /// CalendarProvider). Mobile-only and **host-internal**: built in the
    /// cal-ffi layer over a native bridge (not a dlopen plugin), so its
    /// `plugin_id` is `None` like [`Self::Local`]. Its account is
    /// device-local and is never written to the sync log.
    DeviceCalendar,
    /// Zoom videoconference adapter (DESIGN.md §11). Currently
    /// a stub — the trait impl returns `VcError::Unsupported`
    /// until the REST layer lands.
    Zoom,
    /// Microsoft Teams videoconference adapter. Shares the
    /// OAuth token of [`Self::MicrosoftGraph`].
    Teams,
    /// Google Meet videoconference adapter. Shares the OAuth
    /// refresh token of [`Self::Google`].
    Meet,
    /// Cisco WebEx videoconference adapter (dedicated OAuth
    /// flow).
    Webex,
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
            AdapterKind::DeviceCalendar => "device_calendar",
            AdapterKind::Zoom => "zoom",
            AdapterKind::Teams => "teams",
            AdapterKind::Meet => "meet",
            AdapterKind::Webex => "webex",
        }
    }

    /// The canonical reverse-DNS plugin id that serves this kind, or `None`
    /// for kinds with no plugin (just `Local`, which is host-internal). The one
    /// shared source of the kind→plugin map for both desktop + mobile (e.g. to
    /// resolve an account's manifest capabilities or its plugin-loaded status).
    pub fn plugin_id(self) -> Option<&'static str> {
        Some(match self {
            AdapterKind::Local => return None,
            // Host-internal, built in cal-ffi over a native bridge — no plugin.
            AdapterKind::DeviceCalendar => return None,
            AdapterKind::Caldav => "com.aperio.cal-adapter-caldav",
            AdapterKind::Ical => "com.aperio.cal-adapter-ical",
            AdapterKind::Google => "com.aperio.cal-adapter-google",
            AdapterKind::MicrosoftGraph => "com.aperio.cal-adapter-microsoft-graph",
            AdapterKind::Ews => "com.aperio.cal-adapter-ews",
            AdapterKind::Vikunja => "com.aperio.cal-adapter-vikunja",
            AdapterKind::Todoist => "com.aperio.cal-adapter-todoist",
            AdapterKind::Zoom => "com.aperio.vc-adapter-zoom",
            AdapterKind::Teams => "com.aperio.vc-adapter-teams",
            AdapterKind::Meet => "com.aperio.vc-adapter-meet",
            AdapterKind::Webex => "com.aperio.vc-adapter-webex",
        })
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
            "device_calendar" => AdapterKind::DeviceCalendar,
            "zoom" => AdapterKind::Zoom,
            "teams" => AdapterKind::Teams,
            "meet" => AdapterKind::Meet,
            "webex" => AdapterKind::Webex,
            _ => return None,
        })
    }

    /// True iff this kind is a video-conference adapter
    /// (DESIGN.md §11). Drives the registry's vc-routing path
    /// + the AccountsDialog's "this kind doesn't manage
    /// calendars" rendering.
    pub fn is_videoconference(self) -> bool {
        matches!(
            self,
            AdapterKind::Zoom | AdapterKind::Teams | AdapterKind::Meet | AdapterKind::Webex,
        )
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
            match r? {
                Ok(account) => out.push(account),
                // SKIP, don't propagate. `adapter_kind` is written into this
                // table by the sync applier as an OPAQUE string, so a device
                // running an older build receives rows for kinds it has never
                // heard of the moment the newer device creates one. Failing the
                // whole listing on a single such row took the entire Accounts
                // panel down AND aborted `register_persisted`, i.e. every
                // adapter stopped registering — one unknown account, no
                // calendars at all. The row stays in the table untouched and
                // reappears once this device is updated.
                Err(AccountsError::UnknownKind(kind)) => {
                    tracing::warn!(
                        adapter_kind = %kind,
                        "skipping account with an adapter kind this build does not know \
                         (created by a newer Aperio on another device); update this device \
                         to use it"
                    );
                }
                Err(other) => return Err(other),
            }
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
                rusqlite::Error::QueryReturnedNoRows => AccountsError::NotFound(id.to_string()),
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
            params![
                id,
                adapter_kind.as_str(),
                display_name,
                config_json,
                now,
                now
            ],
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

    /// Change an account's user-visible `display_name`. Returns the
    /// updated row (re-read so the caller has the fresh `updated_at`
    /// for the sync payload). The local account can be renamed — only
    /// deletion is forbidden for it.
    pub fn rename(&self, id: &str, display_name: &str) -> Result<Account, AccountsError> {
        let now = Utc::now().to_rfc3339();
        {
            let conn = self.db.lock().expect("db mutex poisoned");
            let changed = conn.execute(
                "UPDATE accounts SET display_name = ?, updated_at = ? WHERE id = ?",
                params![display_name, now, id],
            )?;
            if changed == 0 {
                return Err(AccountsError::NotFound(id.to_string()));
            }
        }
        self.get(id)?
            .ok_or_else(|| AccountsError::NotFound(id.to_string()))
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
    fn rename_updates_display_name() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let new = repo.create(AdapterKind::Caldav, "Old name", "{}").unwrap();
        let renamed = repo.rename(&new.id, "New name").unwrap();
        assert_eq!(renamed.display_name, "New name");
        assert_eq!(repo.get(&new.id).unwrap().unwrap().display_name, "New name");
    }

    #[test]
    fn renaming_missing_account_errors() {
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let err = repo.rename("does-not-exist", "X").unwrap_err();
        assert!(matches!(err, AccountsError::NotFound(_)));
    }

    #[test]
    fn local_account_can_be_renamed() {
        // Unlike delete, renaming the implicit local account is allowed.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let renamed = repo.rename(LOCAL_ACCOUNT_ID, "My device").unwrap();
        assert_eq!(renamed.display_name, "My device");
    }

    #[test]
    fn list_skips_rows_whose_adapter_kind_this_build_does_not_know() {
        // The sync applier writes `adapter_kind` as an opaque string, so a
        // device running an older build WILL see kinds it cannot parse once a
        // newer device creates one. Before the skip, `list()` returned Err for
        // the whole table — the Accounts panel went empty and
        // `register_persisted` bailed, so NO adapter registered at all.
        let (_tmp, db) = fresh_db();
        let shared = db.shared();
        let repo = AccountsRepo::new(&shared);
        let known = repo.create(AdapterKind::Caldav, "Nextcloud", "{}").unwrap();

        {
            let conn = shared.lock().unwrap();
            conn.execute(
                "INSERT INTO accounts (id, adapter_kind, display_name,
                                       config_json, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
                params![
                    "from-the-future",
                    "quantum_teleconference",
                    "Something newer",
                    "{}",
                    "2026-07-28T00:00:00Z",
                    "2026-07-28T00:00:00Z"
                ],
            )
            .unwrap();
        }

        let listed = repo.list().expect("an unknown kind must not fail the list");
        assert!(listed.iter().any(|a| a.id == known.id));
        assert!(listed.iter().all(|a| a.id != "from-the-future"));

        // A direct lookup by id still reports the truth — the caller asked for
        // that specific row, so silently returning None would be a lie.
        let err = repo.get("from-the-future").unwrap_err();
        assert!(matches!(err, AccountsError::UnknownKind(_)));
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
